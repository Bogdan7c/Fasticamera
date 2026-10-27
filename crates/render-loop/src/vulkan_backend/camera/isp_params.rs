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
//! # Привязка к потоку
//! Подсказки и параметры — данные КАДРОВ одного потока, поэтому годятся только для потока, из
//! кадров которого получены. Поток = сессия камеры = один `Arc<StreamPassport>` (лизинг создаёт его
//! в `begin_stream`, все кадры сессии делят этот `Arc`). Кадр другого потока (другой `Arc`, даже с
//! равным содержимым) или явный `forget_stream` (поток неактивен) стирают подсказки и параметры:
//! чужие метаданные не подставляются никогда (решение 42), чужие уровни/CFA/матрицы — тоже.
//! Описание сенсора — чистая функция содержимого паспорта, его кэш по равенству безопасен.
//!
//! # ВРЕМЕННО: нейтраль от HAL
//! Нейтраль баланса белого берётся из подсказки HAL `NEUTRAL_COLOR_POINT`. Свой AWB появится в
//! сессии 08 (он прочитает статистику из `AwbStatisticsFeed`) и заменит этот источник.

use std::sync::Arc;

use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureMeta, RawFramePassport, StreamPassport};
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::sensor::SensorDescription;

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
    /// HAL не дал `NEUTRAL_COLOR_POINT` (до своего AWB, сессия 08, нейтраль берётся только оттуда).
    #[error("у кадра нет нейтрали NEUTRAL_COLOR_POINT")]
    NoNeutral,
    /// Нейтраль битая (нулевой знаменатель, неположительные значения).
    #[error("нейтраль {0}")]
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
    neutral: [f64; 3],
    dynamic_black_level: Option<[f32; 4]>,
    dynamic_white_level: Option<u32>,
    gamut: DisplayGamut,
}

/// Всё, что выведено из кадров ОДНОГО потока (см. «Привязка к потоку»).
struct StreamBoundState {
    /// Поток, кадрам которого принадлежат подсказки и параметры (идентичность `Arc`).
    stream: Arc<StreamPassport>,
    /// Подсказки последнего кадра этого потока, у которого были метаданные.
    last_hints: Option<CaptureColorHints>,
    /// Действующие параметры и входы, из которых они посчитаны.
    current: Option<(ResolvedInputs, IspParams)>,
    /// Источник нейтрали последнего пересчёта (только для лога смены источника).
    neutral_source: Option<NeutralSource>,
}

impl StreamBoundState {
    fn new(stream: &Arc<StreamPassport>) -> Self {
        Self {
            stream: Arc::clone(stream),
            last_hints: None,
            current: None,
            neutral_source: None,
        }
    }
}

/// Кэш параметров ISP одной сессии показа.
pub struct IspParamsCache {
    /// Каким демозаиком будут считаться кадры (видоискатель — `Preview`, стенд V4 — `Quality`).
    demosaic: DemosaicMethod,
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
    /// входов, что видоискатель).
    pub fn for_demosaic(demosaic: DemosaicMethod) -> Self {
        Self {
            demosaic,
            sensor: None,
            bound: None,
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

    /// Готовит параметры для кадра `passport` под охват экрана `gamut` (см. модуль).
    pub fn prepare(&mut self, passport: &RawFramePassport, gamut: DisplayGamut) -> ParamsDecision {
        // Раздельные заимствования: состояние потока и кэш сенсора — независимые поля.
        let Self {
            demosaic,
            sensor,
            bound,
        } = self;
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
        let outcome = match bound.resolve_if_changed(sensor, *demosaic, &hints, gamut) {
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
        demosaic: DemosaicMethod,
        hints: &CaptureColorHints,
        gamut: DisplayGamut,
    ) -> Result<ParamsOutcome, ParamsFailure> {
        let (neutral, neutral_source) = neutral_from_hints(hints)?;
        if self.neutral_source != Some(neutral_source) {
            // Раз на смену источника, не на кадр: видно в logcat, откуда сейчас берётся цвет.
            tracing::info!(target: "fasticamera::render::camera", ?neutral_source, "источник временной нейтрали");
            self.neutral_source = Some(neutral_source);
        }
        let inputs = ResolvedInputs {
            neutral,
            dynamic_black_level: hints.dynamic_black_level,
            dynamic_white_level: hints.dynamic_white_level,
            gamut,
        };
        if self.current.as_ref().is_some_and(|(current, _)| *current == inputs) {
            return Ok(ParamsOutcome::Unchanged);
        }
        let sensor = sensor_for(sensor, &self.stream)?;
        let frame =
            FrameInputs::from_capture_hints(sensor, hints).map_err(|error| ParamsFailure::Levels(error.to_string()))?;
        let camera_neutral =
            CameraNeutral::new(neutral).map_err(|error| ParamsFailure::InvalidNeutral(error.to_string()))?;
        let controls = ProcessingControls::with_defaults(camera_neutral, demosaic, gamut);
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

/// Откуда взята временная нейтраль баланса белого (до нашего AWB в сессии 08).
///
/// Обе — подсказки того же вендорского AWB для ЭТОГО кадра, чужие кадры и умолчания не подставляются.
/// Факт 07 (телефон): в сессии из одного RAW-потока HAL отдаёт `NEUTRAL_COLOR_POINT` как `0/0` во всех
/// каналах, поэтому нужен запасной источник — коэффициенты `COLOR_CORRECTION_GAINS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NeutralSource {
    /// `ACAMERA_SENSOR_NEUTRAL_COLOR_POINT` — нейтраль в координатах камеры (DNG AsShotNeutral).
    HalNeutralPoint,
    /// `ACAMERA_COLOR_CORRECTION_GAINS` `[R, G_even, G_odd, B]`: коэффициент баланса канала обратен
    /// нейтрали этого канала (серый после усилений одинаков во всех каналах), зелёный — среднее двух.
    HalColorGains,
}

/// Временная нейтраль из подсказок кадра: сначала нейтральная точка HAL, затем его коэффициенты
/// баланса. Нет ни того, ни другого (или оба некорректны) — ошибка с обеими причинами, а не догадка.
fn neutral_from_hints(hints: &CaptureColorHints) -> Result<([f64; 3], NeutralSource), ParamsFailure> {
    let point_problem = match neutral_from_point(hints) {
        Ok(neutral) => return Ok((neutral, NeutralSource::HalNeutralPoint)),
        Err(problem) => problem,
    };
    match neutral_from_gains(hints) {
        Ok(neutral) => Ok((neutral, NeutralSource::HalColorGains)),
        Err(gains_problem) => match (point_problem, gains_problem) {
            // Ни ключа, ни коэффициентов: HAL нейтраль не сообщил вовсе.
            (NeutralProblem::Absent, NeutralProblem::Absent) => Err(ParamsFailure::NoNeutral),
            (point, gains) => Err(ParamsFailure::InvalidNeutral(format!(
                "нейтральная точка: {}; коэффициенты баланса: {}",
                point.describe(),
                gains.describe()
            ))),
        },
    }
}

/// Почему источник нейтрали не годится.
enum NeutralProblem {
    Absent,
    Invalid(String),
}

impl NeutralProblem {
    fn describe(&self) -> String {
        match self {
            Self::Absent => "нет в результате".to_owned(),
            Self::Invalid(reason) => reason.clone(),
        }
    }
}

/// Нейтральная точка HAL: все знаменатели ненулевые, все значения конечные и положительные.
fn neutral_from_point(hints: &CaptureColorHints) -> Result<[f64; 3], NeutralProblem> {
    let rationals = hints.neutral_color_point.ok_or(NeutralProblem::Absent)?;
    let mut values = [0.0; 3];
    for (value, rational) in values.iter_mut().zip(rationals) {
        *value = rational
            .to_f64()
            .filter(|converted| converted.is_finite() && *converted > 0.0)
            .ok_or_else(|| NeutralProblem::Invalid(format!("{rationals:?}")))?;
    }
    Ok(values)
}

/// Нейтраль из коэффициентов баланса HAL: `N_c = 1 / g_c`, для зелёного — среднее двух коэффициентов.
fn neutral_from_gains(hints: &CaptureColorHints) -> Result<[f64; 3], NeutralProblem> {
    let gains = hints.color_gains.ok_or(NeutralProblem::Absent)?.0;
    if !gains.iter().all(|gain| gain.is_finite() && *gain > 0.0) {
        return Err(NeutralProblem::Invalid(format!("{gains:?}")));
    }
    let [red, green_even, green_odd, blue] = gains.map(f64::from);
    Ok([1.0 / red, 2.0 / (green_even + green_odd), 1.0 / blue])
}

#[cfg(test)]
#[path = "isp_params_tests.rs"]
mod tests;
