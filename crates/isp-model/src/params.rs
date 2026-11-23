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
use crate::sensor::SensorDescription;
use crate::tone::{InvalidToneCurve, ToneCurve, ToneCurveSettings};
use crate::white_balance::WhiteBalanceGains;
use camera_contract::frame_passport::CaptureColorHints;

/// Версия набора параметров. Растёт, когда меняется смысл или состав полей: GPU-ISP и
/// сохранённые эталоны сверяют её, чтобы не сравнивать разные модели обработки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IspParamsVersion(pub u32);

/// Текущая версия: 1 — первая модель (сессия 06); 2 — виньетирование по нашей калибровке
/// (карта потока в `LensShadingStep::Apply`, сила коррекции в управлении).
pub const CURRENT_ISP_PARAMS_VERSION: IspParamsVersion = IspParamsVersion(2);

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessingControls {
    /// Нейтраль камеры: какой RGB камеры считать серым (3A, пока — HAL `NEUTRAL_COLOR_POINT`).
    pub white_balance: CameraNeutral,
    pub demosaic: DemosaicMethod,
    pub output_gamut: DisplayGamut,
    pub gamut_compression: GamutCompressionSettings,
    pub tone: ToneCurveSettings,
    /// Остаточная экспозиционная подтяжка (наш AE, 08a): применяется через тон, белый сенсора
    /// остаётся белым (см. [`crate::exposure`]). [`ExposureGain::UNITY`] — без подтяжки.
    pub exposure_gain: ExposureGain,
    /// Коррекция виньетирования: выключена или с силой 0…1 (по умолчанию полная). Сила — покадровое
    /// управление: политика «ослаблять при слабом свете» (бэклог) сможет менять её от кадра к кадру.
    pub lens_shading: LensShadingControl,
}

impl ProcessingControls {
    /// Управление с настройками охвата и тона по умолчанию.
    pub fn with_defaults(white_balance: CameraNeutral, demosaic: DemosaicMethod, output_gamut: DisplayGamut) -> Self {
        Self {
            white_balance,
            demosaic,
            output_gamut,
            gamut_compression: GamutCompressionSettings::default(),
            tone: ToneCurveSettings::default(),
            exposure_gain: ExposureGain::UNITY,
            lens_shading: LensShadingControl::default(),
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
    pub color: ColorTransforms,
    pub gamut_compression: GamutCompression,
    pub tone: ToneCurve,
}

impl IspParams {
    /// Сводит описание сенсора, факты кадра и решения в параметры стадий.
    pub fn resolve(
        sensor: &SensorDescription,
        frame: &FrameInputs,
        controls: &ProcessingControls,
    ) -> Result<Self, ResolveError> {
        let white_balance = WhiteBalanceGains::from_camera_neutral(controls.white_balance);
        Ok(Self {
            version: CURRENT_ISP_PARAMS_VERSION,
            pattern: sensor.pattern(),
            levels: frame.levels,
            lens_shading: LensShadingStep::decide(sensor.lens_shading_map(), controls.lens_shading),
            white_balance,
            demosaic: controls.demosaic,
            color: resolve_color(sensor, controls, &white_balance)?,
            gamut_compression: GamutCompression::resolve(&controls.gamut_compression)?,
            tone: ToneCurve::resolve(&controls.tone.with_exposure_gain(controls.exposure_gain))?,
        })
    }

    /// Веса яркости для статистики ДО баланса белого (см. [`LuminanceWeights`]).
    ///
    /// Тон видит яркость сбалансированного пикселя: `Y = y · (g ⊙ rgb)`, где `g` — усиления
    /// баланса, применённые к мозаике, а `y` — строка Y матрицы «сбалансированная камера → XYZ
    /// D50» ([`ColorTransforms::camera_to_xyz_d50`]; в ней уже учтены `diag(g)⁻¹` и `max(N)`).
    /// Значит веса для несбалансированных каналов: `w_i = y_i · g_i`. Для серого, чей
    /// сбалансированный уровень `b`, получается `Y = b` при любой нейтрали.
    pub fn scene_luminance_weights(&self) -> LuminanceWeights {
        let luminance_row = self.color.camera_to_xyz_d50().rows[1];
        let gains = self.white_balance.as_array();
        LuminanceWeights::new(
            luminance_row[0] * gains[0],
            luminance_row[1] * gains[1],
            luminance_row[2] * gains[2],
        )
    }
}

/// Матрицы цвета: DNG-профиль для нейтрали, затем снятие усилений баланса белого,
/// которые уже применены к мозаике (иначе баланс учитывался бы дважды).
fn resolve_color(
    sensor: &SensorDescription,
    controls: &ProcessingControls,
    white_balance: &WhiteBalanceGains,
) -> Result<ColorTransforms, ResolveError> {
    let camera_to_xyz = sensor.color_profile().camera_to_xyz_d50(controls.white_balance)?;
    // Именно те усиления в f32, что применяет стадия баланса: матрица компенсирует ровно их.
    let applied_gains = white_balance.as_array().map(f64::from);
    let undo_white_balance = Matrix3::diagonal(applied_gains).inverse()?;
    // DNG-матрица переводит саму нейтраль N в D50 с Y = 1, а после баланса серый объект
    // яркости t (RGB камеры t·N) стал (t·max(N), …): усиления max(N)/N. Множитель max(N)
    // возвращает Y = значению сбалансированного серого — экспозиция не зависит от того,
    // как нормирована нейтраль и какой её канал самый яркий.
    let brightest_neutral = controls.white_balance.values().into_iter().fold(f64::MIN, f64::max);
    let balanced_to_xyz = camera_to_xyz
        .matrix
        .mul(&undo_white_balance)
        .mul(&Matrix3::diagonal([brightest_neutral; 3]));
    let xyz_to_scene = xyz_d50_to_scene_linear(controls.output_gamut)?;
    Ok(ColorTransforms {
        camera_to_xyz_d50: balanced_to_xyz.to_f32(),
        xyz_d50_to_scene: xyz_to_scene.to_f32(),
        gamut: controls.output_gamut,
        white: camera_to_xyz.white,
    })
}

#[cfg(test)]
#[path = "params_tests.rs"]
mod tests;
