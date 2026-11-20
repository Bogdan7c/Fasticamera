//! Параметры ISP на кадр: паспорт потока + подсказки кадра → `IspParams`, с кэшем.
//!
//! # Что считается и когда
//! * описание сенсора (`SensorDescription`: мозаика, статические уровни, DNG-матрицы) постоянно на
//!   сессию потока — строится один раз на паспорт потока (тот же `Arc` или равный паспорт);
//! * `IspParams::resolve` (DNG-итерации, обращения матриц, кривые) — только когда изменились входы:
//!   паспорт, нейтраль, динамические уровни или охват экрана. Набор стадий ISP в параметры не входит
//!   (стадии выключаются specialization-константами шейдера), поэтому пересчёта не вызывает;
//! * кадр без метаданных (`FrameCaptureMeta::Missing`, решение 42) берёт подсказки последнего кадра
//!   ТОГО ЖЕ потока, у которого они были; до первого такого кадра параметров нет — кадр не
//!   показывается (цвета не выдумываем, решение 6);
//! * неудачный пересчёт оставляет прежние параметры (если были, того же потока и под тот же охват
//!   экрана) и сообщает причину.
//!
//! # Тон
//! Ручки тона (`viewfinder.tone`: контраст и средние тона экрана) постоянны на жизнь кэша: кэш получает их
//! при создании сессии показа (`with_tone`) и в ключ пересчёта не входят. Средний серый сцены в них не
//! входит никогда: это цель AE (`base_scene_middle_gray`), см. `config::tone`.
//!
//! # Привязка к потоку
//! Подсказки и параметры — данные КАДРОВ одного потока, поэтому годятся только для потока, из
//! кадров которого получены. Поток = сессия камеры = один `Arc<StreamPassport>` (лизинг создаёт его
//! в `begin_stream`, все кадры сессии делят этот `Arc`). Кадр другого потока (другой `Arc`, даже с
//! равным содержимым) или явный `forget_stream` (поток неактивен) стирают подсказки и параметры:
//! чужие метаданные не подставляются никогда (решение 42), чужие уровни/CFA/матрицы — тоже.
//! Описание сенсора — чистая функция содержимого паспорта, его кэш по равенству безопасен.
//!
//! # Нейтраль — только от нашего AWB
//! Белая точка сцены приходит снаружи (`white`: цветность, решение 64) и переводится в нейтраль КОНКРЕТНОГО
//! потока по его DNG-профилю (`DngColorProfile::neutral_for_white`). Подсказки HAL (`NEUTRAL_COLOR_POINT`,
//! `COLOR_CORRECTION_GAINS`) сюда не попадают вообще: из кадра берутся только уровни. Кэш сравнивает белую
//! точно, а решение «белая изменилась заметно» принимает AWB (порог отчёта), поэтому `IspParams::resolve` не
//! идёт на каждый кадр.

use std::sync::Arc;

use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureMeta, RawFramePassport, StreamPassport};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::demosaic::DemosaicMethod;
use isp_model::exposure::ExposureGain;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::sensor::SensorDescription;
use isp_model::tone::ToneCurveSettings;

use crate::config::ViewfinderToneConfig;

/// Откуда взяты подсказки кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintsSource {
    /// Из метаданных этого кадра.
    ThisFrame,
    /// У кадра метаданных нет: подсказки последнего кадра, у которого они были.
    EarlierFrame,
}

/// Почему параметры не удалось посчитать.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParamsFailure {
    /// Паспорт потока не описывает сенсор (не Bayer, нет матриц цвета, битые уровни).
    #[error("описание сенсора: {0}")]
    Sensor(String),
    /// Белая точка не переводится в нейтраль камеры потока (нечисловая, вне допустимого).
    #[error("нейтраль для белой точки: {0}")]
    InvalidNeutral(String),
    /// Уровни кадра не имеют смысла.
    #[error("уровни кадра: {0}")]
    Levels(String),
    /// `IspParams::resolve` отказал (вырожденные матрицы, настройки тона/охвата).
    #[error("IspParams::resolve: {0}")]
    Resolve(String),
}

/// Почему параметров для кадра нет вовсе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamsUnavailable {
    /// В этом потоке ещё не было ни одного кадра с метаданными.
    NoMetadataYet,
    /// Пересчёт не удался, а прежних параметров этого потока под этот охват экрана нет.
    Failed(ParamsFailure),
}

/// Чем кончилась подготовка параметров.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamsOutcome {
    /// Входы изменились (или это первый кадр с метаданными в потоке) — параметры посчитаны заново.
    Resolved,
    /// Входы те же — прежние параметры.
    Unchanged,
    /// Пересчёт не удался — прежние параметры (причина приложена).
    ReusedAfterFailure(ParamsFailure),
    /// Параметров нет: кадр показывать нельзя.
    Unavailable(ParamsUnavailable),
}

/// Итог подготовки параметров для кадра.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamsDecision {
    /// Откуда подсказки (для учёта кадров без метаданных).
    pub hints: HintsSource,
    /// Что с параметрами.
    pub outcome: ParamsOutcome,
}

/// Описание сенсора одного паспорта потока.
struct CachedSensor {
    passport: Arc<StreamPassport>,
    description: Result<SensorDescription, ParamsFailure>,
}

/// Покадровые входы, от которых зависят параметры. Поток в них не входит: всё, что хранит
/// `StreamBoundState`, уже относится к одному потоку.
#[derive(Debug, Clone, PartialEq)]
struct ResolvedInputs {
    /// Белая точка сцены (x, y) — точное сравнение: заметность смены решает AWB.
    white: [f64; 2],
    dynamic_black_level: Option<[f32; 4]>,
    dynamic_white_level: Option<u32>,
    gamut: DisplayGamut,
    /// Остаточная подтяжка нашего AE (часть `IspParams` кадра: снимок повторяет её, решение владельца 1 сессии 08a).
    /// Точное сравнение: единичная подтяжка не меняет ключ, а значит и старый путь бит-в-бит.
    exposure_gain: ExposureGain,
}

/// Всё, что выведено из кадров ОДНОГО потока (см. «Привязка к потоку»).
struct StreamBoundState {
    /// Поток, кадрам которого принадлежат подсказки и параметры (идентичность `Arc`).
    stream: Arc<StreamPassport>,
    /// Подсказки последнего кадра этого потока, у которого были метаданные.
    last_hints: Option<CaptureColorHints>,
    /// Действующие параметры и входы, из которых они посчитаны.
    current: Option<(ResolvedInputs, IspParams)>,
}

impl StreamBoundState {
    fn new(stream: &Arc<StreamPassport>) -> Self {
        Self {
            stream: Arc::clone(stream),
            last_hints: None,
            current: None,
        }
    }
}

/// Входы обработки, постоянные на жизнь кэша (в ключ пересчёта не входят).
#[derive(Debug, Clone, Copy)]
struct ProcessingRecipe {
    /// Каким демозаиком будут считаться кадры (видоискатель — `Preview`, стенд V4 — `Quality`).
    demosaic: DemosaicMethod,
    /// Настройки кривой тона до подтяжки AE: ручки `viewfinder.tone` поверх умолчания `isp-model`.
    tone: ToneCurveSettings,
}

/// Кэш параметров ISP одной сессии показа.
pub struct IspParamsCache {
    recipe: ProcessingRecipe,
    sensor: Option<CachedSensor>,
    /// Подсказки и параметры текущего потока (`None` — потока нет или он забыт).
    bound: Option<StreamBoundState>,
}

impl Default for IspParamsCache {
    /// Кэш видоискателя: демозаик `Preview` (2×2).
    fn default() -> Self {
        Self::for_demosaic(DemosaicMethod::Preview)
    }
}

impl IspParamsCache {
    /// Кэш параметров под демозаик `demosaic` (стенд V4 берёт `Quality` — Menon 2007 — из тех же
    /// входов, что видоискатель) с тоном по умолчанию.
    pub fn for_demosaic(demosaic: DemosaicMethod) -> Self {
        Self {
            recipe: ProcessingRecipe {
                demosaic,
                tone: ViewfinderToneConfig::default().curve_settings(),
            },
            sensor: None,
            bound: None,
        }
    }

    /// Тот же кэш с ручками тона из конфига. Вызывается до первого кадра (при создании сессии показа),
    /// поэтому посчитанных параметров со старым тоном у кэша нет. Значения уже проверены при старте
    /// (`ViewfinderToneConfig::validate`); не построившаяся кривая всё равно дала бы честный отказ кадра.
    pub fn with_tone(self, tone: ViewfinderToneConfig) -> Self {
        Self {
            recipe: ProcessingRecipe {
                tone: tone.curve_settings(),
                ..self.recipe
            },
            bound: None,
            ..self
        }
    }

    /// Действующие параметры текущего потока (`None` — в этом потоке ещё ни разу не посчитаны).
    pub fn current(&self) -> Option<&IspParams> {
        self.bound
            .as_ref()
            .and_then(|bound| bound.current.as_ref())
            .map(|(_, params)| params)
    }

    /// Поток неактивен: подсказки и параметры прежнего потока больше ничьи (см. «Привязка к потоку»).
    /// Описание сенсора остаётся — оно зависит только от содержимого паспорта.
    pub fn forget_stream(&mut self) {
        self.bound = None;
    }

    /// Средний серый сцены базового тона, из которого считаются параметры кадра: цель яркости нашего AE.
    /// Ручки тона его не меняют (`ViewfinderToneConfig::curve_settings` берёт его из умолчания), поэтому
    /// цель AE одна при любом конфиге — число берётся из того же источника, а не дублируется.
    pub fn base_scene_middle_gray() -> f32 {
        ViewfinderToneConfig::default().curve_settings().scene_middle_gray
    }

    /// Готовит параметры для кадра `passport` под охват экрана `gamut` (см. модуль), без остаточной подтяжки.
    pub fn prepare(&mut self, passport: &RawFramePassport, white: Chromaticity, gamut: DisplayGamut) -> ParamsDecision {
        self.prepare_with_exposure_gain(passport, white, ExposureGain::UNITY, gamut)
    }

    /// То же, что [`Self::prepare`], с остаточной подтяжкой нашего AE для этого кадра: пока сенсор догоняет
    /// экспозицию, ISP домножает яркость кадра на `exposure_gain` (через тон, см. `isp_model::exposure`).
    /// Подтяжка входит в ключ кэша: смена `k` пересчитывает параметры, повтор — нет.
    pub fn prepare_with_exposure_gain(
        &mut self,
        passport: &RawFramePassport,
        white: Chromaticity,
        exposure_gain: ExposureGain,
        gamut: DisplayGamut,
    ) -> ParamsDecision {
        // Раздельные заимствования: состояние потока и кэш сенсора — независимые поля.
        let Self { recipe, sensor, bound } = self;
        let bound = bind_to_stream(bound, &passport.stream);
        let (hints, source) = match &passport.capture {
            FrameCaptureMeta::Paired(details) => {
                bound.last_hints = Some(details.color);
                (details.color, HintsSource::ThisFrame)
            }
            FrameCaptureMeta::Missing { .. } => match bound.last_hints {
                Some(hints) => (hints, HintsSource::EarlierFrame),
                // Подсказок этого потока нет — значит, нет и его параметров (они появляются только
                // с подсказками): честное «нет параметров», чужие не берём.
                None => {
                    return ParamsDecision {
                        hints: HintsSource::EarlierFrame,
                        outcome: ParamsOutcome::Unavailable(ParamsUnavailable::NoMetadataYet),
                    };
                }
            },
        };
        let outcome = match bound.resolve_if_changed(sensor, recipe, &hints, white, exposure_gain, gamut) {
            Ok(outcome) => outcome,
            Err(failure) => bound.fallback(failure, gamut),
        };
        ParamsDecision { hints: source, outcome }
    }
}

/// Состояние потока кадра: кадр другого потока (другой `Arc`) начинает его с чистого листа.
fn bind_to_stream<'a>(
    bound: &'a mut Option<StreamBoundState>,
    stream: &Arc<StreamPassport>,
) -> &'a mut StreamBoundState {
    let same_stream = bound.as_ref().is_some_and(|state| Arc::ptr_eq(&state.stream, stream));
    if !same_stream {
        *bound = None;
    }
    bound.get_or_insert_with(|| StreamBoundState::new(stream))
}

impl StreamBoundState {
    /// Пересчёт, если покадровые входы изменились.
    fn resolve_if_changed(
        &mut self,
        sensor: &mut Option<CachedSensor>,
        recipe: &ProcessingRecipe,
        hints: &CaptureColorHints,
        white: Chromaticity,
        exposure_gain: ExposureGain,
        gamut: DisplayGamut,
    ) -> Result<ParamsOutcome, ParamsFailure> {
        let inputs = ResolvedInputs {
            white: [white.x, white.y],
            dynamic_black_level: hints.dynamic_black_level,
            dynamic_white_level: hints.dynamic_white_level,
            gamut,
            exposure_gain,
        };
        if self.current.as_ref().is_some_and(|(current, _)| *current == inputs) {
            return Ok(ParamsOutcome::Unchanged);
        }
        let sensor = sensor_for(sensor, &self.stream)?;
        let frame =
            FrameInputs::from_capture_hints(sensor, hints).map_err(|error| ParamsFailure::Levels(error.to_string()))?;
        // Нейтраль ЭТОГО потока по его DNG-профилю: чужие камеры у одной белой точки имеют разные нейтрали.
        let camera_neutral = sensor
            .color_profile()
            .neutral_for_white(white)
            .map_err(|error| ParamsFailure::InvalidNeutral(error.to_string()))?;
        let controls = ProcessingControls {
            tone: recipe.tone,
            exposure_gain,
            ..ProcessingControls::with_defaults(camera_neutral, recipe.demosaic, gamut)
        };
        let params =
            IspParams::resolve(sensor, &frame, &controls).map_err(|error| ParamsFailure::Resolve(error.to_string()))?;
        self.current = Some((inputs, params));
        Ok(ParamsOutcome::Resolved)
    }

    /// Пересчёт не удался: прежние параметры этого потока, если они посчитаны под тот же охват
    /// экрана (параметры под другой охват дали бы неверные цвета — это уже не «прежние»).
    fn fallback(&self, failure: ParamsFailure, gamut: DisplayGamut) -> ParamsOutcome {
        let reusable = self.current.as_ref().is_some_and(|(inputs, _)| inputs.gamut == gamut);
        if reusable {
            ParamsOutcome::ReusedAfterFailure(failure)
        } else {
            ParamsOutcome::Unavailable(ParamsUnavailable::Failed(failure))
        }
    }
}

/// Описание сенсора паспорта (строится один раз на содержимое паспорта).
fn sensor_for<'a>(
    cache: &'a mut Option<CachedSensor>,
    stream: &Arc<StreamPassport>,
) -> Result<&'a SensorDescription, ParamsFailure> {
    let cached = cache
        .as_ref()
        .is_some_and(|sensor| Arc::ptr_eq(&sensor.passport, stream) || *sensor.passport == **stream);
    if !cached {
        let description = SensorDescription::from_stream_passport(stream, &stream.color_calibration)
            .map_err(|error| ParamsFailure::Sensor(error.to_string()));
        *cache = Some(CachedSensor {
            passport: Arc::clone(stream),
            description,
        });
    }
    match cache.as_ref().map(|sensor| &sensor.description) {
        Some(Ok(description)) => Ok(description),
        Some(Err(failure)) => Err(failure.clone()),
        None => Err(ParamsFailure::Sensor("описание не построено".to_owned())),
    }
}

#[cfg(test)]
#[path = "isp_params_tests.rs"]
mod tests;
