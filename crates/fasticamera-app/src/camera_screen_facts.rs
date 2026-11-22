//! Факты камеры для экрана: перевод статуса движка в нейтральный [`CameraFacts`] (сессия 12).
//!
//! Экран камеры не знает `camera-core` (его граница — нейтральные факты). Этот модуль — единственное место,
//! где статус движка превращается в факты; он чистый (статус на входе → факты на выходе), без часов и ввода-вывода.
//!
//! # Откуда что берётся
//! * присутствие — из состояния движка ([`presence_of_state`]): «стартует» (открывается, настраивается,
//!   перестраивается, закрыта), «работает» (идёт поток), «недоступна» (ошибка, причина переводится в язык экрана);
//! * точка интереса, блокировка — `three_a.point` / `three_a.lock` как есть;
//! * фаза фокуса — `three_a.focus` ([`focus_phase_of`]);
//! * диапазон компенсации экспозиции — возможности **активной** камеры из каталога
//!   (`CameraCapabilities.three_a.exposure_compensation`), а не любой;
//! * геометрия потока (полезная область, активная область, ориентация сенсора) — паспорт потока, собранный
//!   тем же кодом, что у бэкенда (`build_stream_passport`): правило «16:9 занимает верх буфера» живёт в одном месте.
//!   Зеркало экран из геометрии потока больше не берёт (с сессии 13 оно приходит из каталога зума по камере
//!   кадра), поэтому здесь `MirrorMode::None`;
//! * вспышка — [`flash_availability_of`]: доступна, если у активной камеры есть вспышка (14d);
//! * объективы — `lens` статуса движка ([`LensFactsBuilder`]: каталог зума, активная камера, смена, неудача смены).
//!
//! Паспорт потока дорогой (клон калибровки цвета ради трёх полей), поэтому геометрия кэшируется по ИДЕНТИЧНОСТИ
//! сконфигурированного потока ([`StreamGeometryCache`]): пересчёт только при смене потока. Ошибка паспорта не
//! превращается молча в «геометрии нет»: один лог на поток и счётчик ([`FactsDiagnostics::passport_failures`]).
//! `CameraHandle::status()` на каждом кадре остаётся: это снимок готового статуса из ящика движка (клон без ожиданий),
//! а читать частично без правки `camera-core` нельзя.
//!
//! Факты считаются на каждом кадре UI; шлются экрану только при реальном изменении ([`CameraFactsWatcher`]).

use crate::camera_screen_lens_facts::{LensFactsBuilder, LensFactsDiagnostics};
use camera_android::stream_passport::{PassportError, build_stream_passport};
use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capabilities::CameraCapabilities;
use camera_contract::geometry::SensorOrientation;
use camera_contract::ids::CameraId;
use camera_contract::stream::ConfiguredStream;
use camera_contract::three_a::ExposureCompensationSupport;
use camera_core::state::FatalCause;
use camera_core::three_a_status::{FocusOutcome, FocusProgress};
use camera_core::{CameraError, CameraState, CameraStatus};
use camera_screen::exposure::ExposureRangeFact;
use camera_screen::viewfinder_map::{MirrorMode, StreamGeometry};
use camera_screen::{CameraFacts, CameraPresence, FlashAvailability, FocusPhase, UnavailableReason};

/// Факты, когда движка камеры в приложении нет вообще (не поднялся): честное «сбой камеры».
pub fn facts_for_absent_engine() -> CameraFacts {
    CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: UnavailableReason::Failure,
        },
        ..CameraFacts::STARTING
    }
}

/// Перевод статуса движка в факты экрана без кэша (паспорт собирается заново). Для тестов и разовых вызовов;
/// на кадре UI используется [`camera_facts_with_cache`].
pub fn camera_facts_from_status(status: &CameraStatus) -> CameraFacts {
    camera_facts_with_cache(status, &mut FactsCache::default())
}

/// Перевод статуса движка в факты экрана; геометрия потока берётся из кэша.
pub fn camera_facts_with_cache(status: &CameraStatus, cache: &mut FactsCache) -> CameraFacts {
    let active = active_camera_capabilities(status);
    CameraFacts {
        presence: presence_of_state(&status.state),
        point_of_interest: status.three_a.point,
        lock: status.three_a.lock,
        focus: focus_phase_of(status.three_a.focus),
        exposure_range: active.map_or(ExposureRangeFact::NotReported, |caps| {
            exposure_range_of(&caps.three_a.exposure_compensation)
        }),
        stream: cache.geometry.geometry_of(status, active),
        lens: cache.lens.facts_of(&status.lens),
        flash: flash_availability_of(status),
    }
}

/// Вспышка камеры для экрана (14d): доступна тогда и только тогда, когда у **активной** камеры каталога есть
/// вспышка. Нет активной камеры или нет её в каталоге — честное «нет» (инвариант 6): значок и пункт меню скрыты.
pub(crate) fn flash_availability_of(status: &CameraStatus) -> FlashAvailability {
    match active_camera_capabilities(status) {
        Some(capabilities) if capabilities.flash.is_present() => FlashAvailability::Available,
        Some(_) | None => FlashAvailability::Unavailable,
    }
}

/// Всё, что источник фактов помнит между кадрами: кэш геометрии потока и память о неудачах смены объектива.
#[derive(Debug, Default)]
pub struct FactsCache {
    geometry: StreamGeometryCache,
    lens: LensFactsBuilder,
}

impl FactsCache {
    pub fn diagnostics(&self) -> FactsDiagnostics {
        FactsDiagnostics {
            lens: self.lens.diagnostics(),
            ..self.geometry.diagnostics()
        }
    }
}

/// Присутствие камеры по состоянию движка.
pub fn presence_of_state(state: &CameraState) -> CameraPresence {
    match state {
        CameraState::Streaming { .. } => CameraPresence::Live,
        // Поток шёл и перестраивается (смена режима/камеры): картинка вернётся сама — это не «недоступна».
        CameraState::Closed { .. }
        | CameraState::Opening { .. }
        | CameraState::Opened { .. }
        | CameraState::Configuring { .. }
        | CameraState::Reconfiguring { .. } => CameraPresence::Starting,
        CameraState::Error { error } => CameraPresence::Unavailable {
            reason: unavailable_reason_of(error),
        },
    }
}

fn unavailable_reason_of(error: &CameraError) -> UnavailableReason {
    match error {
        CameraError::Recoverable { reason, .. } => screen_reason_of(*reason),
        CameraError::Fatal {
            cause: FatalCause::RecoveryExhausted { reason, .. },
        } => screen_reason_of(*reason),
        CameraError::Fatal { .. } => UnavailableReason::Failure,
    }
}

/// Причина недоступности камеры на языке экрана (экран различает четыре случая).
fn screen_reason_of(reason: CameraUnavailableReason) -> UnavailableReason {
    match reason {
        CameraUnavailableReason::PermissionDenied => UnavailableReason::PermissionDenied,
        CameraUnavailableReason::DisabledByPolicy => UnavailableReason::DisabledByPolicy,
        CameraUnavailableReason::Evicted
        | CameraUnavailableReason::InUseByAnotherClient
        | CameraUnavailableReason::TooManyCamerasOpen => UnavailableReason::InUse,
        CameraUnavailableReason::DeviceFailure
        | CameraUnavailableReason::ServiceFailure
        | CameraUnavailableReason::CameraNotPresent => UnavailableReason::Failure,
    }
}

/// Фаза фокуса для стиля квадрата.
pub fn focus_phase_of(progress: FocusProgress) -> FocusPhase {
    match progress {
        // Фиксированный фокус, ручная дистанция и непрерывный AF — «точки нет, камера сама».
        FocusProgress::FixedLens | FocusProgress::Manual | FocusProgress::Continuous { .. } => FocusPhase::Continuous,
        FocusProgress::Scanning { .. } => FocusPhase::Scanning,
        FocusProgress::Settled { outcome, .. } => match outcome {
            FocusOutcome::Focused => FocusPhase::Focused,
            FocusOutcome::NotFocused => FocusPhase::NotFocused,
            // Итога нет: время вышло либо тап не дошёл до HAL.
            FocusOutcome::TimedOut | FocusOutcome::TriggerNotDelivered => FocusPhase::NoResult,
        },
    }
}

/// Диапазон компенсации экспозиции камеры в EV.
pub fn exposure_range_of(support: &ExposureCompensationSupport) -> ExposureRangeFact {
    match support {
        ExposureCompensationSupport::NotSupported => ExposureRangeFact::Unsupported,
        ExposureCompensationSupport::Supported {
            min_steps,
            max_steps,
            step_ev,
        } => {
            // Битый шаг (нулевой знаменатель) диапазон определить не даёт: «камера не сообщила», не выдумка.
            let Some(step) = step_ev.to_f64() else {
                return ExposureRangeFact::NotReported;
            };
            if *min_steps == 0 && *max_steps == 0 {
                return ExposureRangeFact::Unsupported;
            }
            ExposureRangeFact::Supported {
                min_ev: (f64::from(*min_steps) * step) as f32,
                max_ev: (f64::from(*max_steps) * step) as f32,
            }
        }
    }
}

/// Камера, к которой относится статус: настраиваемая, работающая или открытая.
fn active_camera_id(state: &CameraState) -> Option<&CameraId> {
    match state {
        CameraState::Opening { camera } | CameraState::Opened { camera } => Some(camera),
        CameraState::Configuring { stream } => Some(&stream.camera),
        CameraState::Reconfiguring { target } => Some(&target.camera),
        CameraState::Streaming { stream } => Some(&stream.stream.camera),
        CameraState::Closed { .. } | CameraState::Error { .. } => None,
    }
}

/// Возможности активной камеры из каталога статуса; `None` — камеры нет или её нет в каталоге.
fn active_camera_capabilities(status: &CameraStatus) -> Option<&CameraCapabilities> {
    let camera = active_camera_id(&status.state)?;
    status.catalog.camera(camera).map(|descriptor| &descriptor.capabilities)
}

/// Ориентация сенсора активной камеры по каталогу статуса. Это факт HAL, а не перевод экрана: сценарий `screen`
/// проверяет им, ПРАВИЛЬНО ли экран перевёл касание в точку сенсора.
pub fn sensor_orientation_of_active_camera(status: &CameraStatus) -> Option<SensorOrientation> {
    active_camera_capabilities(status).map(|capabilities| capabilities.sensor_orientation)
}

/// Счётчики кэша геометрии (уходят в отчёт экрана).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FactsDiagnostics {
    /// Сколько раз паспорт собирался (смена потока), а не кадров.
    pub passport_builds: u64,
    /// Сколько потоков не дали паспорта (нет RAW, 16:9 не влезает): у них геометрии нет, и это не тишина.
    pub passport_failures: u64,
    /// Счётчики перевода статуса объективов.
    pub lens: LensFactsDiagnostics,
}

/// Паспорт одного сконфигурированного потока: итог сборки геометрии.
#[derive(Debug)]
struct CachedGeometry {
    /// Для какого потока посчитано (идентичность: камера, буфер, раскладка, механизм частоты).
    stream: ConfiguredStream,
    outcome: Result<StreamGeometry, PassportError>,
}

/// Кэш геометрии потока: паспорт собирается только при смене сконфигурированного потока.
#[derive(Debug, Default)]
pub struct StreamGeometryCache {
    last: Option<CachedGeometry>,
    diagnostics: FactsDiagnostics,
}

impl StreamGeometryCache {
    pub fn diagnostics(&self) -> FactsDiagnostics {
        self.diagnostics
    }

    /// Геометрия потока; только пока поток идёт (до этого буфера, в котором считать, ещё нет).
    fn geometry_of(&mut self, status: &CameraStatus, active: Option<&CameraCapabilities>) -> Option<StreamGeometry> {
        let CameraState::Streaming { stream } = &status.state else {
            return None;
        };
        // Камеры нет в каталоге — считать не из чего; в кэш это не пишем: каталог может пополниться.
        let capabilities = active?;
        if let Some(cached) = &self.last
            && cached.stream == *stream
        {
            return cached.outcome.ok();
        }
        let outcome = Self::build_geometry(stream, capabilities);
        self.diagnostics.passport_builds += 1;
        if let Err(error) = &outcome {
            // Паспорт не собирается, если у камеры нет RAW или 16:9 не влезает в буфер: геометрии нет, экран не
            // гадает. Лог один раз на поток (повтор кадров попадает в кэш выше), плюс счётчик в отчёте.
            self.diagnostics.passport_failures += 1;
            tracing::error!(target: "fasticamera::camera_screen", %error, camera = ?stream.stream.camera, "паспорт потока не собран: геометрии для экрана нет");
        }
        self.last = Some(CachedGeometry {
            stream: stream.clone(),
            outcome,
        });
        outcome.ok()
    }

    fn build_geometry(
        stream: &ConfiguredStream,
        capabilities: &CameraCapabilities,
    ) -> Result<StreamGeometry, PassportError> {
        let passport = build_stream_passport(&stream.stream, capabilities, stream.frame_rate_mechanism)?;
        Ok(StreamGeometry {
            valid_area: passport.valid_area,
            active_array_size: capabilities.active_array.size,
            sensor_orientation: passport.sensor_orientation,
            // Зеркало решает каталог зума по камере кадра (решение 115), а не геометрия потока.
            mirror: MirrorMode::None,
        })
    }
}

/// Откуда источник UI берёт актуальные факты камеры каждый кадр. Узкий контракт: источник не знает,
/// что за ним — настоящий движок или его отсутствие.
pub trait CameraFactsSource: Send {
    fn current_facts(&mut self) -> CameraFacts;

    /// Счётчики источника (ошибки паспорта потока и т. п.) для отчёта экрана.
    fn diagnostics(&self) -> FactsDiagnostics;
}

/// Факты от настоящего движка: свежий снимок статуса без ожидания.
pub struct EngineFactsSource {
    handle: camera_core::CameraHandle,
    cache: FactsCache,
}

impl EngineFactsSource {
    pub fn new(handle: camera_core::CameraHandle) -> Self {
        Self {
            handle,
            cache: FactsCache::default(),
        }
    }
}

impl CameraFactsSource for EngineFactsSource {
    fn current_facts(&mut self) -> CameraFacts {
        camera_facts_with_cache(&self.handle.status(), &mut self.cache)
    }

    fn diagnostics(&self) -> FactsDiagnostics {
        self.cache.diagnostics()
    }
}

/// Движка нет: экран видит «сбой камеры» (явный итог), а не вечное «стартует».
pub struct AbsentEngineFactsSource;

impl CameraFactsSource for AbsentEngineFactsSource {
    fn current_facts(&mut self) -> CameraFacts {
        facts_for_absent_engine()
    }

    fn diagnostics(&self) -> FactsDiagnostics {
        FactsDiagnostics::default()
    }
}

/// Итог наблюдения: изменились ли факты с прошлого раза.
#[derive(Debug, Clone, PartialEq)]
pub enum FactsChange {
    /// Факты новые (первое наблюдение или отличаются от прежних): их надо отправить экрану.
    Changed(CameraFacts),
    /// Факты те же: экрану ничего не шлём.
    Unchanged,
}

/// Помнит последние отправленные факты и отдаёт `Changed` только при реальной разнице.
#[derive(Debug, Default)]
pub struct CameraFactsWatcher {
    last_sent: Option<CameraFacts>,
}

impl CameraFactsWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Сравнивает факты с последними отправленными.
    pub fn observe(&mut self, facts: CameraFacts) -> FactsChange {
        if self.last_sent.as_ref() == Some(&facts) {
            return FactsChange::Unchanged;
        }
        self.last_sent = Some(facts.clone());
        FactsChange::Changed(facts)
    }
}

#[cfg(test)]
#[path = "camera_screen_facts_tests.rs"]
mod tests;
