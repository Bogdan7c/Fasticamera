//! Снимок наблюдаемого состояния для сценария `screen`: что видно снаружи потока рендера про экран камеры и про
//! движок камеры в один момент времени.
//!
//! Сценарий судит шаги по ПРИРАЩЕНИЯМ счётчиков между снимком в начале шага и текущим ([`ProbeDelta`]), а не по
//! абсолютным числам: так шаги не зависят друг от друга и от того, сколько жестов было до них.
//!
//! # Единственный адаптер к состоянию экрана
//! Отчёт экрана ([`ScreenReport`]) несёт счётчики (жесты по видам, намерения камере, ввод), миниатюру и снимок
//! наблюдаемого состояния ([`ScreenStateSnapshot`]: открытая панель, плашка блокировки, шаги EV, режим, затвор).
//! «Меню открыто», «плашка видна» сценарий читает из снимка напрямую; счётчики остаются для проверки связи с HAL.

use camera_contract::geometry::SensorOrientation;
use camera_contract::three_a::{AeAfLock, PointOfInterest};
use camera_core::CameraStatus;
use camera_screen::CameraPresence;
use camera_screen::shutter::ThumbnailState;

use crate::camera_screen_facts::sensor_orientation_of_active_camera;
use crate::camera_screen_service::CameraScreenService;
use crate::camera_screen_source::{ScreenReport, ScreenStateSnapshot};

/// Почему снимок не получился.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeUnavailable {
    /// Экран ещё не нарисовал ни одного кадра: отчёта нет.
    NoScreenReport,
}

/// Снимок: накопительные счётчики и текущие значения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenProbe {
    // ---- жесты, распознанные экраном (накопительные) ----
    pub taps: u64,
    pub long_presses: u64,
    pub pans: u64,
    pub button_presses: u64,
    pub gestures_cancelled: u64,
    // ---- намерения камере от исполнителя эффектов (накопительные) ----
    pub camera_intents_sent: u64,
    /// Очередь движка полна + движок остановлен + движка нет: намерение потеряно.
    pub camera_intents_lost: u64,
    // ---- сами команды дошли до HAL: счётчики движка (накопительные) ----
    pub focus_triggers_sent: u64,
    pub focus_trigger_failures: u64,
    pub results_with_unknown_origin: u64,
    // ---- фейк-снимок (накопительные) ----
    pub capture_requests: u64,
    // ---- текущие значения ----
    pub presence_live: bool,
    pub lock: AeAfLock,
    /// Точка интереса фокуса и замера в движке: КУДА ушёл последний тап (а не только «что ушёл»).
    pub point_of_interest: PointOfInterest,
    /// Ориентация сенсора активной камеры (факт HAL): нужна, чтобы независимо вывести ожидаемую точку сенсора.
    pub sensor_orientation: Option<SensorOrientation>,
    /// Желаемая компенсация экспозиции В ДВИЖКЕ (`three_a.controls`): камера её приняла в очередь, но применил ли
    /// её HAL, статус не говорит — «дошло до движка», не «применено HAL».
    pub exposure_bias_ev: f32,
    pub thumbnail_has_last: bool,
    /// Наблюдаемое состояние экрана (прямое доказательство панели, плашки, шагов EV, режима, затвора).
    pub screen: ScreenStateSnapshot,
    pub frames_produced: u64,
}

impl ScreenProbe {
    /// Снимок по отчёту экрана и статусу движка. `capture_requests` — число принятых запросов снимка (не зависит от режима: фейк или настоящая служба).
    pub fn read(screen: &CameraScreenService, status: &CameraStatus) -> Result<Self, ProbeUnavailable> {
        let report = screen.report().ok_or(ProbeUnavailable::NoScreenReport)?;
        Ok(Self::from_parts(&report, status, screen.capture_requests_accepted()))
    }

    /// Сборка из готовых частей (отдельно от сервиса — чтобы проверять без маршрутизатора).
    pub fn from_parts(report: &ScreenReport, status: &CameraStatus, capture_requests: u64) -> Self {
        let gestures = report.diagnostics.gestures;
        let camera = report.effects.camera;
        let three_a = &status.three_a;
        Self {
            taps: gestures.taps,
            long_presses: gestures.long_presses,
            pans: gestures.pans,
            button_presses: gestures.button_presses,
            gestures_cancelled: gestures.cancelled,
            camera_intents_sent: camera.sent,
            camera_intents_lost: camera.queue_full + camera.engine_stopped + camera.no_engine,
            focus_triggers_sent: three_a.counters.focus_triggers_sent,
            focus_trigger_failures: three_a.counters.focus_trigger_failures,
            results_with_unknown_origin: three_a.counters.results_with_unknown_origin,
            capture_requests,
            presence_live: report.presence == CameraPresence::Live,
            lock: three_a.lock,
            point_of_interest: three_a.point,
            sensor_orientation: sensor_orientation_of_active_camera(status),
            exposure_bias_ev: three_a.controls.exposure_bias.ev(),
            thumbnail_has_last: matches!(report.thumbnail, ThumbnailState::Last { .. }),
            screen: report.state,
            frames_produced: report.diagnostics.frames_produced,
        }
    }
}

/// Приращения счётчиков от снимка `before` до `now` и текущие значения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeDelta {
    pub taps: u64,
    pub long_presses: u64,
    pub pans: u64,
    pub button_presses: u64,
    pub camera_intents: u64,
    pub camera_intents_lost: u64,
    pub focus_triggers: u64,
    pub focus_trigger_failures: u64,
    pub results_with_unknown_origin: u64,
    pub capture_requests: u64,
    pub lock: AeAfLock,
    pub point_of_interest: PointOfInterest,
    pub sensor_orientation: Option<SensorOrientation>,
    pub exposure_bias_ev: f32,
    pub thumbnail_has_last: bool,
    /// Текущее (не приращение) состояние экрана.
    pub screen: ScreenStateSnapshot,
}

impl ProbeDelta {
    pub fn between(before: &ScreenProbe, now: &ScreenProbe) -> Self {
        // Счётчики не убывают; saturating — на случай пересоздания экрана посреди сценария (счётчики с нуля).
        let grew = |pick: fn(&ScreenProbe) -> u64| pick(now).saturating_sub(pick(before));
        Self {
            taps: grew(|probe| probe.taps),
            long_presses: grew(|probe| probe.long_presses),
            pans: grew(|probe| probe.pans),
            button_presses: grew(|probe| probe.button_presses),
            camera_intents: grew(|probe| probe.camera_intents_sent),
            camera_intents_lost: grew(|probe| probe.camera_intents_lost),
            focus_triggers: grew(|probe| probe.focus_triggers_sent),
            focus_trigger_failures: grew(|probe| probe.focus_trigger_failures),
            results_with_unknown_origin: grew(|probe| probe.results_with_unknown_origin),
            capture_requests: grew(|probe| probe.capture_requests),
            lock: now.lock,
            point_of_interest: now.point_of_interest,
            sensor_orientation: now.sensor_orientation,
            exposure_bias_ev: now.exposure_bias_ev,
            thumbnail_has_last: now.thumbnail_has_last,
            screen: now.screen,
        }
    }
}
