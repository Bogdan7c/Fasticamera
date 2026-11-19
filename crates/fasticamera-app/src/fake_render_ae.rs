//! Подставной «AE рендера» для тестов приложения. Только для тестов.
//!
//! В тестах приложения настоящего рендера нет, а наш AE живёт на их стыке: движок ждёт предложений,
//! рендер читает указания. Этот помощник играет рендер — тем же языком, что и настоящий
//! (`camera_contract::auto_exposure`): читает указание движка, плавно «ведёт цель» к яркости сцены
//! и кладёт предложения в ящик с толчком (`Searching` → `Settled`; при блокировке — `Holding` с
//! замороженной целью, смена сцены её не двигает).
//!
//! Метку кадра берёт из последнего результата HAL в статусе движка: так предложение всегда «новее»
//! начала потока, как у настоящего рендера, который меряет кадры потока.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use camera_contract::auto_exposure::{
    ExposureProduct, ExposureProposal, MeasurementQuality, MeteredArea, ProposalConvergence,
};
use camera_contract::three_a::AeAfLock;

use crate::auto_exposure_link::ExposureControllerKind;
use crate::camera_test_support::FakeCameraRig;

/// Пауза потока между шагами (реальное время).
const STEP_PAUSE: Duration = Duration::from_millis(2);
/// Сцена по умолчанию, с·ISO: комнатный свет (10 мс при ISO 200). Внутри пределов камеры с запасом на ±1 EV.
pub const DEFAULT_SCENE_SECONDS_ISO: f64 = 2.0;
/// Во сколько раз начальная цель темнее сцены: чтобы был виден путь `Searching` → `Settled`.
const START_BELOW_SCENE: f64 = 2.0;
/// Доля пути к сцене (в EV) за один шаг сглаживания.
const SMOOTHING_FRACTION: f64 = 0.5;
/// Допуск «цель совпала со сценой», EV.
const SETTLED_TOLERANCE_EV: f64 = 0.02;

/// Сглаживание цели и блокировка — чистая логика, без потоков.
struct MeterState {
    smoothed: f64,
    held: Option<f64>,
}

impl MeterState {
    fn new(scene: f64) -> Self {
        Self {
            smoothed: scene / START_BELOW_SCENE,
            held: None,
        }
    }

    /// Один шаг: цель и сходимость при данной сцене и блокировке.
    fn step(&mut self, scene: f64, lock: AeAfLock) -> (f64, ProposalConvergence) {
        if lock == AeAfLock::Unlocked {
            self.held = None;
        }
        if let Some(held) = self.held {
            return (held, ProposalConvergence::Holding);
        }
        let distance_ev = (scene / self.smoothed).log2();
        let settled = distance_ev.abs() < SETTLED_TOLERANCE_EV;
        if settled {
            self.smoothed = scene;
        } else {
            self.smoothed *= (distance_ev * SMOOTHING_FRACTION).exp2();
        }
        if settled && lock == AeAfLock::Locked {
            self.held = Some(self.smoothed);
            return (self.smoothed, ProposalConvergence::Holding);
        }
        let convergence = if settled {
            ProposalConvergence::Settled
        } else {
            ProposalConvergence::Searching
        };
        (self.smoothed, convergence)
    }
}

/// Что считается новым предложением: метка кадра меняется каждый шаг, публиковать из-за неё не нужно.
#[derive(Clone, Copy, PartialEq)]
struct ProposalKey {
    target_bits: u64,
    convergence: ProposalConvergence,
    area: MeteredArea,
}

/// Поток подставного рендера. Живёт, пока жив объект.
pub struct FakeRenderAe {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    scene: Arc<Mutex<f64>>,
}

impl FakeRenderAe {
    /// Запускает рендер на ящиках стенда с сценой по умолчанию.
    pub fn start(rig: &FakeCameraRig) -> Self {
        Self::start_with_scene(rig, DEFAULT_SCENE_SECONDS_ISO)
    }

    pub fn start_with_scene(rig: &FakeCameraRig, scene_seconds_iso: f64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let scene = Arc::new(Mutex::new(scene_seconds_iso));
        let handle = rig.handle();
        let link = rig.auto_exposure.clone();
        let (thread_stop, thread_scene) = (Arc::clone(&stop), Arc::clone(&scene));
        let join = thread::spawn(move || {
            let Some(render_end) = link.render_end(ExposureControllerKind::Own) else {
                return;
            };
            let mut meter = MeterState::new(scene_seconds_iso);
            let mut last_key: Option<ProposalKey> = None;
            while !thread_stop.load(Ordering::SeqCst) {
                thread::sleep(STEP_PAUSE);
                // Без результатов потока мерить нечего (метки кадра нет).
                let Some(frame) = handle
                    .status()
                    .three_a
                    .last_result
                    .and_then(|result| result.sensor_timestamp)
                else {
                    continue;
                };
                let directive = render_end.directives.latest().map_or(
                    camera_contract::auto_exposure::ExposureDirective::DEFAULT,
                    |(_, value)| value,
                );
                let scene_now = *thread_scene.lock().unwrap_or_else(PoisonError::into_inner);
                let (target, convergence) = meter.step(scene_now, directive.lock);
                let Ok(target) = ExposureProduct::from_seconds_iso(target) else {
                    continue;
                };
                let area = MeteredArea::for_point(directive.metering);
                let key = ProposalKey {
                    target_bits: target.seconds_iso().to_bits(),
                    convergence,
                    area,
                };
                if last_key == Some(key) {
                    continue;
                }
                last_key = Some(key);
                render_end.proposals.publish(ExposureProposal {
                    measured_frame: frame,
                    target,
                    metered_area: area,
                    convergence,
                    quality: MeasurementQuality::Measured,
                });
            }
        });
        Self {
            stop,
            join: Some(join),
            scene,
        }
    }

    /// Общий доступ к текущей сцене: подставной поток рендера отдаёт её сценарию как сырой замер сцены.
    pub fn scene_probe(&self) -> Arc<Mutex<f64>> {
        Arc::clone(&self.scene)
    }

    /// «Смена сцены»: теперь сцене нужно столько света, с·ISO.
    pub fn change_scene(&self, scene_seconds_iso: f64) {
        *self.scene.lock().unwrap_or_else(PoisonError::into_inner) = scene_seconds_iso;
    }
}

impl Drop for FakeRenderAe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_walks_to_the_scene_then_settles() {
        let mut meter = MeterState::new(2.0);
        let (first, convergence) = meter.step(2.0, AeAfLock::Unlocked);
        assert!(first < 2.0 && convergence == ProposalConvergence::Searching);
        let mut last = (first, convergence);
        for _ in 0..40 {
            last = meter.step(2.0, AeAfLock::Unlocked);
        }
        assert_eq!(last, (2.0, ProposalConvergence::Settled));
    }

    #[test]
    fn locked_meter_freezes_the_target_against_a_scene_change() {
        let mut meter = MeterState::new(2.0);
        for _ in 0..40 {
            meter.step(2.0, AeAfLock::Unlocked);
        }
        assert_eq!(meter.step(2.0, AeAfLock::Locked), (2.0, ProposalConvergence::Holding));
        assert_eq!(
            meter.step(8.0, AeAfLock::Locked),
            (2.0, ProposalConvergence::Holding),
            "под блокировкой цель не следует за сценой"
        );
        let (released, _) = meter.step(8.0, AeAfLock::Unlocked);
        assert!(released > 2.0, "после снятия блокировки цель снова идёт к сцене");
    }
}
