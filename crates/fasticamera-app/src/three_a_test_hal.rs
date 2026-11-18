//! Подставной HAL автоматики 3A для функциональных тестов сценария `3a`. Только для тестов.
//!
//! Один поток владеет ВСЕМИ результатами съёмки (номера кадров свои, подряд; метки сенсора синтетические):
//! `FakeBackendControl::complete_frames` не годится — его результаты без `AE_STATE` и дистанции фокуса.
//! Поведение выводится из того, что движок реально запросил у бэкенда (`repeating_request`, журнал вызовов):
//! * ручной план `Sensor` — HAL отдаёт ровно его выдержку и ISO (как на телефоне, зонд 08);
//! * вендорский AE — «энергия кадра» `BASE × 2^(шаги компенсации / 6)`, блокировка → `AE_STATE = Locked`;
//! * тап (`SubmitFocusTrigger`) — кадр с меткой триггера и несколькими кадрами `ActiveScan`, затем итог;
//! * серия (`SubmitBracket`) — кадры `Burst` по индексам подряд, с выдержками серии.
//!
//! Неисправности [`HalFaults`] включаются по одной на каждое проверяемое правило сценария.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::three_a::{
    AeState, AfState, BurstId, ExposureBracket, PointRequestId, RequestOrigin, ThreeAResultState,
};
use camera_core::backend::BackendEvent;
use camera_core::fake::{BackendCall, FakeBackendControl};
use camera_core::request_controls::{ExposurePlan, FocusPlan, RequestControls};

use crate::camera_test_support::FakeCameraRig;

/// Номер первого кадра HAL (не ноль: проверяет, что сценарий не полагается на «с нуля»).
const FIRST_FRAME: i64 = 1_000;
/// Интервал между метками кадров, нс (60 к/с).
const FRAME_INTERVAL_NANOS: i64 = 16_666_667;
/// Пауза потока HAL между кадрами (реальное время); метки от неё не зависят.
const HAL_STEP: Duration = Duration::from_millis(4);
/// Энергия кадра вендорского AE без компенсации: 4 мс при ISO 200, нс·ISO.
const BASE_PRODUCT: f64 = 4_000_000.0 * 200.0;
/// ISO вендорского AE в тесте.
const AUTO_ISO: u32 = 200;
/// Шагов компенсации на 1 EV (шаг HAL 1/6 EV).
const STEPS_PER_EV: f64 = 6.0;
/// Сколько кадров после триггера HAL «сканирует».
const SCAN_FRAMES: u32 = 3;

/// Включаемые неисправности. По умолчанию HAL здоров.
#[derive(Clone, Copy, Default)]
pub struct HalFaults {
    /// В результатах нет `AE_STATE`.
    pub no_ae_state: bool,
    /// HAL игнорирует ручной план: отдаёт 8 мс / ISO 100.
    pub ignores_manual_exposure: bool,
    /// Компенсация экспозиции не действует.
    pub ignores_exposure_bias: bool,
    /// Блокировка не берётся: `AE_STATE` остаётся `Converged`.
    pub ae_never_locks: bool,
    /// Скан фокуса не кончается никогда.
    pub focus_never_settles: bool,
    /// Между кадрами серии теряется номер кадра.
    pub burst_loses_a_frame_number: bool,
    /// Итог фокуса — `NOT_FOCUSED_LOCKED` (честный итог HAL, не отказ).
    pub focus_reports_not_focused: bool,
}

/// Счётчики, которые читают подставной рендер и тесты.
#[derive(Default)]
pub struct HalStats {
    /// Сколько кадров серии HAL выдал за всё время.
    pub burst_frames_emitted: AtomicU64,
}

/// Поток подставного HAL. Живёт, пока жив объект.
pub struct SimulatedHal {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    pub stats: Arc<HalStats>,
}

impl SimulatedHal {
    pub fn start(rig: &FakeCameraRig, faults: HalFaults) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(HalStats::default());
        let thread_stop = Arc::clone(&stop);
        let thread_stats = Arc::clone(&stats);
        let control = Arc::clone(&rig.control);
        let join = thread::spawn(move || {
            let mut hal = HalState::new(faults, thread_stats);
            while !thread_stop.load(Ordering::SeqCst) {
                hal.step(&control);
                thread::sleep(HAL_STEP);
            }
        });
        Self {
            stop,
            join: Some(join),
            stats,
        }
    }
}

impl Drop for SimulatedHal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Состояние HAL между кадрами.
struct HalState {
    faults: HalFaults,
    stats: Arc<HalStats>,
    next_frame: i64,
    seen_calls: usize,
    triggers: VecDeque<PointRequestId>,
    bursts: VecDeque<(BurstId, u32)>,
    brackets: HashMap<BurstId, ExposureBracket>,
    scanning: bool,
    scan_frames_left: u32,
}

impl HalState {
    fn new(faults: HalFaults, stats: Arc<HalStats>) -> Self {
        Self {
            faults,
            stats,
            next_frame: FIRST_FRAME,
            seen_calls: 0,
            triggers: VecDeque::new(),
            bursts: VecDeque::new(),
            brackets: HashMap::new(),
            scanning: false,
            scan_frames_left: 0,
        }
    }

    /// Один кадр: читает новые вызовы движка, выбирает происхождение и шлёт результат.
    fn step(&mut self, control: &FakeBackendControl) {
        let (Some((_, controls)), Some(session)) = (control.repeating_request(), control.current_session_generation())
        else {
            return;
        };
        self.absorb_new_calls(control);
        let (origin, exposure) = self.next_origin_and_exposure(&controls);
        let (exposure_time, iso) = exposure;
        let (af_state, distance) = self.focus_state(&controls, origin);
        let frame = self.take_frame_number(origin);
        let result = CaptureResultMeta {
            frame_number: FrameNumber(frame),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(frame * FRAME_INTERVAL_NANOS)),
            exposure_time: Some(exposure_time),
            sensitivity_iso: Some(iso),
            reported_frame_duration: None,
            rolling_shutter_skew: None,
            three_a: ThreeAResultState {
                af_state,
                ae_state: self.ae_state(&controls),
                focus_distance_diopters: Some(distance),
                ..ThreeAResultState::unreported(origin)
            },
        };
        control.send_event(BackendEvent::CaptureCompleted { session, result });
    }

    fn absorb_new_calls(&mut self, control: &FakeBackendControl) {
        let calls = control.calls();
        for call in calls.iter().skip(self.seen_calls) {
            match call {
                BackendCall::SubmitFocusTrigger { request, .. } => self.triggers.push_back(*request),
                BackendCall::SubmitBracket { bracket, burst, .. } => {
                    self.brackets.insert(*burst, bracket.clone());
                    let frames = u32::try_from(bracket.frames().len()).unwrap_or(u32::MAX);
                    self.bursts.extend((0..frames).map(|index| (*burst, index)));
                }
                _ => {}
            }
        }
        self.seen_calls = calls.len();
    }

    /// Происхождение следующего кадра и его экспозиция (выдержка, ISO).
    fn next_origin_and_exposure(&mut self, controls: &RequestControls) -> (RequestOrigin, (Duration, u32)) {
        if let Some((burst, index)) = self.bursts.pop_front() {
            self.stats.burst_frames_emitted.fetch_add(1, Ordering::SeqCst);
            let frame = self.brackets.get(&burst).and_then(|bracket| {
                bracket
                    .frames()
                    .get(usize::try_from(index).unwrap_or(usize::MAX))
                    .copied()
            });
            let exposure = frame.map_or((Duration::from_millis(8), 100), |frame| {
                (frame.exposure_time.duration(), frame.iso.value())
            });
            return (RequestOrigin::Burst { burst, index }, exposure);
        }
        let origin = match self.triggers.pop_front() {
            Some(request) => {
                self.scanning = true;
                self.scan_frames_left = SCAN_FRAMES;
                RequestOrigin::FocusTrigger { request }
            }
            None => RequestOrigin::Repeating,
        };
        (origin, self.repeating_exposure(controls))
    }

    fn repeating_exposure(&self, controls: &RequestControls) -> (Duration, u32) {
        match controls.exposure {
            ExposurePlan::Sensor { exposure_time, iso } if !self.faults.ignores_manual_exposure => {
                (exposure_time.duration(), iso.value())
            }
            ExposurePlan::Sensor { .. } => (Duration::from_millis(8), 100),
            ExposurePlan::VendorAuto { compensation_steps, .. } => {
                let steps = if self.faults.ignores_exposure_bias {
                    0
                } else {
                    compensation_steps
                };
                let product = BASE_PRODUCT * 2.0_f64.powf(f64::from(steps) / STEPS_PER_EV);
                let exposure_ns = product / f64::from(AUTO_ISO);
                (Duration::from_nanos(exposure_ns.round() as u64), AUTO_ISO)
            }
        }
    }

    fn ae_state(&self, controls: &RequestControls) -> Option<AeState> {
        if self.faults.no_ae_state {
            return None;
        }
        Some(match controls.exposure {
            ExposurePlan::Sensor { .. } => AeState::Inactive,
            ExposurePlan::VendorAuto { lock, .. } => {
                let locked = lock == camera_contract::three_a::AeAfLock::Locked && !self.faults.ae_never_locks;
                if locked { AeState::Locked } else { AeState::Converged }
            }
        })
    }

    /// Состояние AF и дистанция объектива; дистанция зависит от точки тапа (разные точки — разные сцены).
    fn focus_state(&mut self, controls: &RequestControls, origin: RequestOrigin) -> (Option<AfState>, f32) {
        match controls.focus {
            FocusPlan::SingleShot { region } => {
                let center = region.center();
                let distance = 1.0 + 2.0 * (1.0 - center.x()) + 0.5 * center.y();
                let state = if matches!(origin, RequestOrigin::FocusTrigger { .. }) {
                    AfState::ActiveScan
                } else if self.scanning && (self.scan_frames_left > 0 || self.faults.focus_never_settles) {
                    self.scan_frames_left = self.scan_frames_left.saturating_sub(1);
                    AfState::ActiveScan
                } else {
                    self.scanning = false;
                    if self.faults.focus_reports_not_focused {
                        AfState::NotFocusedLocked
                    } else {
                        AfState::FocusedLocked
                    }
                };
                (Some(state), distance)
            }
            FocusPlan::Continuous { .. } => {
                self.scanning = false;
                (Some(AfState::PassiveFocused), 0.5)
            }
            FocusPlan::FixedLens | FocusPlan::Manual { .. } => (Some(AfState::Inactive), 0.5),
        }
    }

    fn take_frame_number(&mut self, origin: RequestOrigin) -> i64 {
        let skip_one =
            self.faults.burst_loses_a_frame_number && matches!(origin, RequestOrigin::Burst { index: 2, .. });
        if skip_one {
            self.next_frame += 1;
        }
        let frame = self.next_frame;
        self.next_frame += 1;
        frame
    }
}
