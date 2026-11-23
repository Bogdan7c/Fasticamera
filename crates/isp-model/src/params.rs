//! Параметры обработки кадра: одинаковые для эталона на CPU и для GPU-ISP (07, 14).
//!
//! Разделение ответственности:
//! - [`SensorDescription`] — постоянное на сессию (мозаика, уровни, DNG-профиль, вендор);
//! - [`FrameInputs`] — факты конкретного кадра (динамические уровни);
//! - [`ProcessingControls`] — решения: баланс белого (3A, сессия 08), демозаик, гамма выхода,
//!   сжатие охвата, тон.
//!
//! [`IspParams::resolve`] сводит их в готовые на кадр значения: усиления, матрицы в f32,
//! разрешённые кривые. GPU получает ровно эти числа, поэтому паритет сравнивает одно и то же,
//! а DNG-математика (итерации белой точки, обращения матриц) не попадает в шейдер.

use crate::cfa::BayerPattern;
use crate::color::chromaticity::xyz_d50_to_scene_linear;
use crate::color::dng::{CameraNeutral, ColorProfileError, WhiteEstimate};
use crate::color::matrix::{Matrix3, PixelMatrix, SingularMatrix};
use crate::demosaic::DemosaicMethod;
pub use crate::exposure::{ExposureGain, InvalidExposureGain, LuminanceWeights};
use crate::gamut::{GamutCompression, GamutCompressionSettings, InvalidGamutCompression};
use crate::image::DisplayGamut;
use crate::lens_shading::{LensShadingControl, LensShadingStep};
use crate::levels::{InvalidLevels, NormalizationLevels};
use crate::look::white::{WhiteShift, WhiteShiftLimit, WhiteShiftSkipReason, shift_white};
use crate::look::{LookControls, LookParams, ResolveLookError};
use crate::output::{OutputProfile, UnsupportedOutputProfile};
use crate::sensor::SensorDescription;
use crate::tone::aces::InvalidAces;
use crate::tone::agx_settings::InvalidAgx;
use crate::tone::base::{InvalidToneBase, ResolvedToneBase, ToneBase, WhitePolicy};
use crate::tone::filmic::InvalidFilmic;
use crate::tone::{InvalidToneCurve, ToneCurve, ToneCurveSettings};
use crate::white_balance::WhiteBalanceGains;
use camera_contract::frame_passport::CaptureColorHints;

/// Версия набора параметров. Растёт, когда меняется смысл или состав полей: GPU-ISP и
/// сохранённые эталоны сверяют её, чтобы не сравнивать разные модели обработки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IspParamsVersion(pub u32);

/// Текущая версия: 1 — первая модель (сессия 06); 2 — виньетирование по нашей калибровке
/// (карта потока в `LensShadingStep::Apply`, сила коррекции в управлении); 3 — основа тона AgX в рабочем
/// пространстве Rec. 2020 со своим белым точки (14a, решения 101, 176, 177): состав параметров кадра расширен
/// (`tone_base`, `white_policy`), умолчание — AgX.
pub const CURRENT_ISP_PARAMS_VERSION: IspParamsVersion = IspParamsVersion(3);

/// Факты конкретного кадра.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameInputs {
    pub levels: NormalizationLevels,
}

impl FrameInputs {
    /// Факты кадра из цветовых подсказок результата запроса. Карту виньетирования HAL не берём:
    /// коррекция идёт по нашей калибровке потока (она в описании сенсора).
    pub fn from_capture_hints(sensor: &SensorDescription, hints: &CaptureColorHints) -> Result<Self, InvalidLevels> {
        Ok(Self {
            levels: sensor.frame_levels(hints)?,
        })
    }
}

/// Решения обработки на кадр.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessingControls {
    /// Нейтраль камеры: какой RGB камеры считать серым (3A, пока — HAL `NEUTRAL_COLOR_POINT`).
    pub white_balance: CameraNeutral,
    pub demosaic: DemosaicMethod,
    /// Профиль выхода (решение 184): основные цвета, кривая, разрядность, рендеринг. Охват выхода берётся из него
    /// ([`OutputProfile::gamut`]); не поддержанный профиль (Log, HDR) отвергает [`IspParams::resolve`].
    pub output_profile: OutputProfile,
    pub gamut_compression: GamutCompressionSettings,
    pub tone: ToneCurveSettings,
    /// Остаточная экспозиционная подтяжка (наш AE, 08a): применяется через тон, белый сенсора
    /// остаётся белым (см. [`crate::exposure`]). [`ExposureGain::UNITY`] — без подтяжки.
    pub exposure_gain: ExposureGain,
    /// Коррекция виньетирования: выключена или с силой 0…1 (по умолчанию полная). Сила — покадровое
    /// управление: политика «ослаблять при слабом свете» (бэклог) сможет менять её от кадра к кадру.
    pub lens_shading: LensShadingControl,
    /// Основа тона: прежняя кривая или AgX (решения 101, 176, 177). Один выбор для видоискателя и снимка.
    pub tone_base: ToneBase,
    /// Вид картинки: ручки, модель вида и таблица (решения 98, 99, 178). Умолчание нейтрально — основа без
    /// вида бит в бит. Теплота и оттенок сдвигают белую точку ТОЛЬКО для рисования (см. [`crate::look::white`]):
    /// `white_balance` здесь остаётся честной нейтралью AWB.
    pub look: LookControls,
}

impl ProcessingControls {
    /// Управление с настройками охвата и тона по умолчанию и SDR-профилем выхода с охватом `output_gamut`
    /// ([`OutputProfile::sdr`]: sRGB или Display P3). Другой профиль — [`Self::with_output_profile`].
    pub fn with_defaults(white_balance: CameraNeutral, demosaic: DemosaicMethod, output_gamut: DisplayGamut) -> Self {
        Self {
            white_balance,
            demosaic,
            output_profile: OutputProfile::sdr(output_gamut),
            gamut_compression: GamutCompressionSettings::default(),
            tone: ToneCurveSettings::default(),
            exposure_gain: ExposureGain::UNITY,
            lens_shading: LensShadingControl::default(),
            tone_base: ToneBase::default(),
            look: LookControls::default(),
        }
    }

    /// Те же управления с видом `look`.
    #[must_use]
    pub fn with_look(self, look: LookControls) -> Self {
        Self { look, ..self }
    }

    /// Те же управления с профилем выхода `output_profile` (в том числе ещё не поддержанным: его отвергнет
    /// [`IspParams::resolve`]).
    #[must_use]
    pub fn with_output_profile(self, output_profile: OutputProfile) -> Self {
        Self { output_profile, ..self }
    }

    /// Те же управления с прежней основой тона (вариант сравнения до 14b): для эталонов и тестов, где
    /// важна прежняя картинка бит в бит, а не умолчание.
    #[must_use]
    pub fn with_legacy_tone(self) -> Self {
        Self {
            tone_base: ToneBase::Legacy,
            ..self
        }
    }
}

/// Матрицы цвета кадра в f32 (попиксельные), гамма выхода и белая точка, из которой они
/// получены. Поля закрыты: матрица «XYZ → экран» и гамма, под которую она посчитана, меняются
/// только вместе — в [`IspParams::resolve`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTransforms {
    camera_to_xyz_d50: PixelMatrix,
    xyz_d50_to_scene: PixelMatrix,
    gamut: DisplayGamut,
    white: WhiteEstimate,
}

impl ColorTransforms {
    /// RGB камеры **после** баланса белого → XYZ D50: `CameraToXYZ · diag(усиления)⁻¹ · max(N)`.
    /// Сбалансированный серый `b·(1,1,1)` даёт `b·XYZ(D50)` с `Y = b` при любом масштабе нейтрали.
    pub fn camera_to_xyz_d50(&self) -> &PixelMatrix {
        &self.camera_to_xyz_d50
    }

    /// XYZ D50 → линейный свет в основных цветах экрана (Брэдфорд D50 → D65 внутри).
    pub fn xyz_d50_to_scene(&self) -> &PixelMatrix {
        &self.xyz_d50_to_scene
    }

    /// Гамма выхода, под которую посчитана [`Self::xyz_d50_to_scene`].
    pub fn gamut(&self) -> DisplayGamut {
        self.gamut
    }

    /// Белая точка сцены по DNG (диагностика: температура, сходимость).
    pub fn white(&self) -> WhiteEstimate {
        self.white
    }
}

/// Параметры кадра нельзя собрать.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ResolveError {
    #[error("цвет камеры → XYZ: {0}")]
    ColorProfile(#[from] ColorProfileError),
    #[error("матрица баланса белого или гаммы выхода вырождена: {0}")]
    SingularColorMatrix(#[from] SingularMatrix),
    #[error("сжатие охвата: {0}")]
    GamutCompression(#[from] InvalidGamutCompression),
    #[error("тон-кривая: {0}")]
    ToneCurve(#[from] InvalidToneCurve),
    #[error("основа AgX: {0}")]
    Agx(#[from] InvalidAgx),
    #[error("основа ACES: {0}")]
    Aces(#[from] InvalidAces),
    #[error("основа filmic: {0}")]
    Filmic(#[from] InvalidFilmic),
    #[error("профиль выхода: {0}")]
    OutputProfile(#[from] UnsupportedOutputProfile),
    #[error("вид картинки: {0}")]
    Look(#[from] ResolveLookError),
}

impl From<InvalidToneBase> for ResolveError {
    /// Ошибка модели основы попадает в свой вариант (текст и различимость сохраняются).
    fn from(error: InvalidToneBase) -> Self {
        match error {
            InvalidToneBase::Agx(error) => Self::Agx(error),
            InvalidToneBase::Aces(error) => Self::Aces(error),
            InvalidToneBase::Filmic(error) => Self::Filmic(error),
        }
    }
}

/// Всё, что нужно стадиям на кадр (см. модуль).
#[derive(Debug, Clone, PartialEq)]
pub struct IspParams {
    pub version: IspParamsVersion,
    pub pattern: BayerPattern,
    pub levels: NormalizationLevels,
    pub lens_shading: LensShadingStep,
    pub white_balance: WhiteBalanceGains,
    pub demosaic: DemosaicMethod,
    /// Профиль выхода кадра (решение 184): поддержанный (иначе `resolve` отказывает). Охват выхода в `color`
    /// выведен из него и всегда ему равен.
    pub output_profile: OutputProfile,
    pub color: ColorTransforms,
    pub gamut_compression: GamutCompression,
    /// Прежняя кривая тона: заполнена всегда (при основе AgX остаётся вариантом для сравнения).
    pub tone: ToneCurve,
    /// Выбранная основа тона (разрешённая): `Legacy` — прежняя цепочка, `Agx`, `Aces`, `Filmic` — модели в `W` (решения 101, 176, 182).
    pub tone_base: ResolvedToneBase,
    /// Обрезка пересвета и источник белого; выводится из основы (решение 177), отдельной ручки нет.
    pub white_policy: WhitePolicy,
    /// Вид картинки, разрешённый на кадр (решение 178). Теплота и оттенок уже учтены в `color` и
    /// `white_balance` (рисование); AE видит несдвинутые веса ([`Self::scene_luminance_weights`]).
    pub look: LookParams,
    /// Веса яркости для AE: считаются по НЕсдвинутой нейтрали AWB (инвариант 1: AE меряет честную базу).
    pub metering_weights: LuminanceWeights,
}

impl IspParams {
    /// Сводит описание сенсора, факты кадра и решения в параметры стадий. Основу тона берёт из
    /// `controls.tone_base`: для моделей в `W` (AgX, ACES, filmic) подтяжка AE `controls.exposure_gain` двигает серый кривой
    /// модели (`scene_middle_gray / k`), белый точки остаётся белым. `Legacy` даёт результат побитно прежний.
    pub fn resolve(
        sensor: &SensorDescription,
        frame: &FrameInputs,
        controls: &ProcessingControls,
    ) -> Result<Self, ResolveError> {
        // Профиль выхода — первым: не поддержанный (Log, HDR) отвергается до любых вычислений.
        controls.output_profile.check_supported()?;
        let output_gamut = controls.output_profile.gamut();
        let working = controls.tone_base.working_primaries(output_gamut);
        let mut look = LookParams::resolve(&controls.look, &working)?;
        // Единый шов ручек тона (решение 182): настройки модели после ручек; нуль ручки — настройки бит в бит.
        let knobbed = controls.tone_base.with_knobs(&controls.tone, &look.tone);
        look.report.unsupported_tone_knobs = knobbed.unsupported;
        let tone_base = knobbed.base.resolve(controls.exposure_gain, output_gamut)?;
        // Цвет по честной нейтрали AWB: из него веса AE и белая точка сцены. Рисование (теплота, оттенок) — ниже.
        let scene_gains = WhiteBalanceGains::from_camera_neutral(controls.white_balance);
        let scene_color = resolve_color(sensor, controls.white_balance, output_gamut, &scene_gains)?;
        let metering_weights = luminance_weights(&scene_color, &scene_gains);
        let (white_balance, color) = if look.white_shift.is_none() {
            (scene_gains, scene_color)
        } else {
            match render_with_shifted_white(sensor, &scene_color, look.white_shift, output_gamut) {
                Ok(shifted) => {
                    look.report.white_shift_limit = shifted.limit;
                    (shifted.gains, shifted.color)
                }
                // Ручка не вправе сделать кадр непригодным: рисуем честной белой, причина — в отчёте вида.
                Err(reason) => {
                    look.report.white_shift_limit = WhiteShiftLimit::NotApplied(reason);
                    (scene_gains, scene_color)
                }
            }
        };
        Ok(Self {
            version: CURRENT_ISP_PARAMS_VERSION,
            pattern: sensor.pattern(),
            levels: frame.levels,
            lens_shading: LensShadingStep::decide(sensor.lens_shading_map(), controls.lens_shading),
            white_balance,
            demosaic: controls.demosaic,
            output_profile: controls.output_profile,
            color,
            gamut_compression: GamutCompression::resolve(&controls.gamut_compression)?,
            tone: ToneCurve::resolve(&knobbed.legacy_curve.with_exposure_gain(controls.exposure_gain))?,
            white_policy: tone_base.white_policy(),
            tone_base,
            look,
            metering_weights,
        })
    }

    /// Тонкая обёртка над [`Self::resolve`] для вызовов, где основа задана отдельным аргументом:
    /// подменяет `controls.tone_base` и больше ничего не делает (логика разрешения одна).
    pub fn resolve_with_base(
        sensor: &SensorDescription,
        frame: &FrameInputs,
        controls: &ProcessingControls,
        base: &ToneBase,
    ) -> Result<Self, ResolveError> {
        Self::resolve(
            sensor,
            frame,
            &ProcessingControls {
                tone_base: *base,
                ..controls.clone()
            },
        )
    }

    /// Веса яркости для статистики ДО баланса белого (см. [`LuminanceWeights`]).
    ///
    /// Тон видит яркость сбалансированного пикселя: `Y = y · (g ⊙ rgb)`, где `g` — усиления
    /// баланса, применённые к мозаике, а `y` — строка Y матрицы «сбалансированная камера → XYZ
    /// D50» ([`ColorTransforms::camera_to_xyz_d50`]; в ней уже учтены `diag(g)⁻¹` и `max(N)`).
    /// Значит веса для несбалансированных каналов: `w_i = y_i · g_i`. Для серого, чей
    /// сбалансированный уровень `b`, получается `Y = b` при любой нейтрали.
    pub fn scene_luminance_weights(&self) -> LuminanceWeights {
        self.metering_weights
    }
}

/// Веса яркости `w_i = y_i · g_i` по строке Y матрицы «сбалансированная камера → XYZ D50» и усилениям `g`.
fn luminance_weights(color: &ColorTransforms, gains: &WhiteBalanceGains) -> LuminanceWeights {
    let luminance_row = color.camera_to_xyz_d50().rows[1];
    let gains = gains.as_array();
    LuminanceWeights::new(
        luminance_row[0] * gains[0],
        luminance_row[1] * gains[1],
        luminance_row[2] * gains[2],
    )
}

/// Усиления и матрицы рисования для сдвинутой белой (теплота, оттенок) плюс то, насколько сдвиг удалось применить.
struct ShiftedRendering {
    gains: WhiteBalanceGains,
    color: ColorTransforms,
    limit: WhiteShiftLimit,
}

/// Считает рисование для сдвинутой белой. Любой сбой (белая вне таблицы, нет нейтрали, вырожденные матрицы)
/// отдаётся причиной, а не ошибкой кадра: вызывающий откатывается на несдвинутую белую.
fn render_with_shifted_white(
    sensor: &SensorDescription,
    scene_color: &ColorTransforms,
    shift: WhiteShift,
    output_gamut: DisplayGamut,
) -> Result<ShiftedRendering, WhiteShiftSkipReason> {
    let rendering = shift_white(scene_color.white.white, shift).map_err(WhiteShiftSkipReason::ShiftOutsideTable)?;
    let neutral = sensor
        .color_profile()
        .neutral_for_white(rendering.white)
        .map_err(WhiteShiftSkipReason::ShiftedWhiteHasNoNeutral)?;
    let gains = WhiteBalanceGains::from_camera_neutral(neutral);
    let rendered =
        resolve_color(sensor, neutral, output_gamut, &gains).map_err(WhiteShiftSkipReason::ShiftedColorUnresolvable)?;
    Ok(ShiftedRendering {
        gains,
        // Белая точка СЦЕНЫ (диагностика) остаётся честной, сдвинуты только усиления и матрицы рисования.
        color: ColorTransforms {
            white: scene_color.white,
            ..rendered
        },
        limit: rendering.limit,
    })
}

/// Матрицы цвета: DNG-профиль для нейтрали, затем снятие усилений баланса белого,
/// которые уже применены к мозаике (иначе баланс учитывался бы дважды).
fn resolve_color(
    sensor: &SensorDescription,
    neutral: CameraNeutral,
    output_gamut: DisplayGamut,
    white_balance: &WhiteBalanceGains,
) -> Result<ColorTransforms, ResolveError> {
    let camera_to_xyz = sensor.color_profile().camera_to_xyz_d50(neutral)?;
    // Именно те усиления в f32, что применяет стадия баланса: матрица компенсирует ровно их.
    let applied_gains = white_balance.as_array().map(f64::from);
    let undo_white_balance = Matrix3::diagonal(applied_gains).inverse()?;
    // DNG-матрица переводит саму нейтраль N в D50 с Y = 1, а после баланса серый объект
    // яркости t (RGB камеры t·N) стал (t·max(N), …): усиления max(N)/N. Множитель max(N)
    // возвращает Y = значению сбалансированного серого — экспозиция не зависит от того,
    // как нормирована нейтраль и какой её канал самый яркий.
    let brightest_neutral = neutral.values().into_iter().fold(f64::MIN, f64::max);
    let balanced_to_xyz = camera_to_xyz
        .matrix
        .mul(&undo_white_balance)
        .mul(&Matrix3::diagonal([brightest_neutral; 3]));
    let xyz_to_scene = xyz_d50_to_scene_linear(output_gamut)?;
    Ok(ColorTransforms {
        camera_to_xyz_d50: balanced_to_xyz.to_f32(),
        xyz_d50_to_scene: xyz_to_scene.to_f32(),
        gamut: output_gamut,
        white: camera_to_xyz.white,
    })
}

#[cfg(test)]
#[path = "params_tests.rs"]
mod tests;
