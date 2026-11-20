//! Общий стенд тестов смены объектива: движок с ящиком намерений и короткие сценарии.

use camera_contract::fixtures::camera_id;
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{LensIntent, LensIntentChannel, ZoomGesture, ZoomRatio};

use super::*;
use crate::lens_status::LensSwitchState;
use crate::lens_wiring::LensWiring;

/// Интервал между кадрами fake (60 к/с).
pub(super) const FRAME: Duration = Duration::from_micros(16_667);

pub(super) struct LensRig {
    pub(super) rig: Rig,
    pub(super) intents: LensIntentChannel,
}

impl LensRig {
    /// Движок с ящиком намерений, каталог прочитан, камера не открыта.
    pub(super) fn closed() -> Self {
        let intents = LensIntentChannel::new();
        let config = CameraEngineConfig::default();
        let fake = FakeBackendControl::new();
        let collector = Arc::new(Collector::default());
        let listener: Arc<dyn BackendEventListener> = Arc::clone(&collector) as Arc<dyn BackendEventListener>;
        let backend = fake.backend(BackendEventSink::new(listener));
        let wiring = ThreeAWiring::vendor(&config.three_a);
        let engine = CameraEngine::with_wiring(config, AppPresence::Foreground, backend, wiring)
            .with_lens_wiring(LensWiring::new(intents.clone()));
        let mut rig = Rig::from_parts(engine, fake, collector);
        rig.engine.load_catalog(rig.now);
        rig.fresh_events();
        rig.fake.take_calls();
        Self { rig, intents }
    }

    /// Основная камера стримит 4:3@60, пришли кадры: `Steady(Wide)`. Журналы очищены.
    pub(super) fn streaming_on_wide() -> Self {
        let mut lens = Self::closed();
        lens.rig.send(CameraCommand::Open { target: wide() });
        lens.rig.send(CameraCommand::Configure {
            mode: mode_raw10_4x3_at(60),
        });
        lens.rig.send(CameraCommand::StartStreaming {
            intent: CaptureIntent::Preview,
        });
        assert_eq!(lens.rig.state_name(), "streaming");
        lens.frames(3);
        assert_eq!(lens.switch_state(), LensSwitchState::Steady { role: LensRole::Wide });
        lens.rig.fresh_events();
        lens.rig.fake.take_calls();
        lens
    }

    /// Плавное намерение (колесо, щипок): с гистерезисом. Движок читает его, как после толчка.
    pub(super) fn intent(&mut self, facing: LensFacing, ratio: f32) {
        self.intent_with(facing, ratio, ZoomGesture::Continuous);
    }

    /// Явное намерение (тап по кнопке зума, переворот): без гистерезиса.
    pub(super) fn intent_discrete(&mut self, facing: LensFacing, ratio: f32) {
        self.intent_with(facing, ratio, ZoomGesture::Discrete);
    }

    fn intent_with(&mut self, facing: LensFacing, ratio: f32, gesture: ZoomGesture) {
        self.intents.publish(LensIntent {
            facing,
            ratio: ZoomRatio::new(ratio).expect("валидный коэффициент"),
            gesture,
        });
        self.rig.engine.refresh_lens_intent(self.rig.now);
        self.rig.pump();
    }

    /// Только прочитать ящик (без новой публикации).
    pub(super) fn poll(&mut self) {
        self.rig.engine.refresh_lens_intent(self.rig.now);
        self.rig.pump();
    }

    pub(super) fn frames(&mut self, count: u32) {
        self.rig.fake.complete_frames(count, FRAME);
        self.rig.pump();
    }

    pub(super) fn switch_state(&self) -> LensSwitchState {
        self.rig.engine.status().lens.switch
    }

    pub(super) fn open_cameras(&self) -> Vec<String> {
        self.rig
            .fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                BackendCall::OpenDevice { camera, .. } => Some(camera.to_string()),
                _ => None,
            })
            .collect()
    }
}

pub(super) fn ultra_wide_id() -> camera_contract::ids::CameraId {
    camera_id("3")
}
