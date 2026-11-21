//! Общий стенд тестов тёплых камер (13b): движок с ящиком намерений, каталог OnePlus 11
//! (при желании с синтетическим телевиком), удобные намерения «с колесом».

use camera_contract::fixtures::{camera_id, main_wide_camera, oneplus11_visible_cameras};
use camera_contract::ids::{CameraId, LensFacing};
use camera_contract::zoom::{LensIntent, StandbyDemand, ZoomGesture, ZoomRatio};

use super::lens_support::LensRig;
use super::*;
use crate::backend_standby::StandbySlotId;
use crate::lens_wiring::LensWiring;
use crate::standby_status::{StandbySlotState, StandbyStatus};

/// Камера 9 — синтетический телевик ≈ 2× (как в тестах шкалы зума).
pub(super) fn tele_id() -> CameraId {
    camera_id("9")
}

pub(super) fn wide_id() -> CameraId {
    camera_id("2")
}

pub(super) fn ultra_wide_id() -> CameraId {
    camera_id("3")
}

pub(super) fn slot(index: u8) -> StandbySlotId {
    StandbySlotId::new(index)
}

fn synthetic_tele() -> camera_contract::capabilities::CameraDescriptor {
    let mut tele = main_wide_camera();
    tele.id = tele_id();
    tele.capabilities.optics.focal_lengths_mm = vec![7.08];
    tele.capabilities.optics.sensor_physical_size.width_mm = 5.248;
    tele.capabilities.optics.sensor_physical_size.height_mm = 3.942;
    tele
}

/// Что стенд кладёт в каталог и конфиг.
#[derive(Clone, Copy)]
pub(super) struct StandbyRigSetup {
    pub(super) with_tele: bool,
    pub(super) max_cameras: usize,
}

impl Default for StandbyRigSetup {
    fn default() -> Self {
        Self {
            with_tele: false,
            max_cameras: 1,
        }
    }
}

pub(super) struct StandbyRig {
    pub(super) lens: LensRig,
}

impl StandbyRig {
    /// Основная стримит 4:3@60, кадры пришли, журналы вызовов и событий очищены.
    pub(super) fn streaming_on_wide(setup: StandbyRigSetup) -> Self {
        let intents = camera_contract::zoom::LensIntentChannel::new();
        let mut config = CameraEngineConfig::default();
        config.standby.max_cameras = setup.max_cameras;
        let fake = FakeBackendControl::new();
        let mut cameras = oneplus11_visible_cameras();
        if setup.with_tele {
            cameras.push(synthetic_tele());
        }
        fake.set_cameras(cameras);
        let collector = Arc::new(Collector::default());
        let listener: Arc<dyn BackendEventListener> = Arc::clone(&collector) as Arc<dyn BackendEventListener>;
        let backend = fake.backend(BackendEventSink::new(listener));
        let wiring = ThreeAWiring::vendor(&config.three_a);
        let engine = CameraEngine::with_wiring(config, AppPresence::Foreground, backend, wiring)
            .with_lens_wiring(LensWiring::new(intents.clone()));
        let mut rig = Rig::from_parts(engine, fake, collector);
        rig.engine.load_catalog(rig.now);
        let mut lens = LensRig { rig, intents };
        lens.rig.send(CameraCommand::Open { target: wide() });
        lens.rig.send(CameraCommand::Configure {
            mode: mode_raw10_4x3_at(60),
        });
        lens.rig.send(CameraCommand::StartStreaming {
            intent: CaptureIntent::Preview,
        });
        assert_eq!(lens.rig.state_name(), "streaming");
        lens.frames(3);
        lens.rig.fresh_events();
        lens.rig.fake.take_calls();
        Self { lens }
    }

    /// Намерение с явным спросом на тёплые (колесо/щипок: `Continuous`, тап: `Discrete`).
    pub(super) fn intent(&mut self, ratio: f32, gesture: ZoomGesture, standby: StandbyDemand) {
        self.lens.intents.publish(
            LensIntent::without_standby(
                LensFacing::Back,
                ZoomRatio::new(ratio).expect("валидный коэффициент"),
                gesture,
            )
            .with_standby(standby),
        );
        self.lens.poll();
    }

    /// Колесо на коэффициенте: плавный жест со спросом «греть соседей».
    pub(super) fn wheel_at(&mut self, ratio: f32) {
        self.intent(ratio, ZoomGesture::Continuous, StandbyDemand::KeepNeighbourWarm);
    }

    /// Колесо закрыто: спроса нет.
    pub(super) fn wheel_closed_at(&mut self, ratio: f32) {
        self.intent(ratio, ZoomGesture::Continuous, StandbyDemand::NotNeeded);
    }

    /// Тап по кнопке зума при действующем спросе (тёплая уже есть — её и повысят).
    pub(super) fn tap_while_warm(&mut self, ratio: f32) {
        self.intent(ratio, ZoomGesture::Discrete, StandbyDemand::KeepNeighbourWarm);
    }

    /// Тёплая в ячейке 0 отдала кадры: готова к повышению.
    pub(super) fn make_standby_ready(&mut self, index: u8) {
        self.lens.rig.fake.complete_standby_frames(slot(index));
        self.lens.rig.pump();
    }

    pub(super) fn standby(&self) -> StandbyStatus {
        self.lens.rig.engine.status().lens.standby
    }

    pub(super) fn slot_state(&self, index: u8) -> StandbySlotState {
        self.standby().slots[usize::from(index)].state.clone()
    }

    pub(super) fn calls(&mut self) -> Vec<&'static str> {
        call_names(&self.lens.rig.fake.take_calls())
    }
}

/// Полностью ручная экспозиция: ISO и выдержка заданы.
pub(super) fn fully_manual(iso: u32, shutter: Duration) -> camera_contract::control::ManualControl {
    use camera_contract::control::{Control, ExposureTime, Iso, ManualControl};
    ManualControl {
        iso: Control::Manual(Iso::new(iso).expect("ISO")),
        shutter: Control::Manual(ExposureTime::new(shutter).expect("выдержка")),
        ..ManualControl::AUTOMATIC
    }
}
