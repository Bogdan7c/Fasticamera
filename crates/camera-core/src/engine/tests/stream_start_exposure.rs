//! Ожидаемая экспозиция нового потока (13b, решения 118 и 126): движок публикует её рендеру в указании
//! (`ExposureDirective::stream_start`) при обмене тёплой камеры и при обычном переоткрытии.
//!
//! Стенд: настоящий движок с нашим AE (настоящий ящик предложений), fake-бэкенд, каталог OnePlus 11.

use std::sync::atomic::{AtomicU32, Ordering};

use camera_contract::auto_exposure::{
    CameraTag, CarriedSceneExposure, ExposureDirective, ExposureProduct, ExposureProposal, ExposureProposalChannel,
    MeasurementQuality, MeteredArea, ProposalConvergence, RefreshSignal,
};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::fixtures::{camera_id, main_wide_camera, oneplus11_visible_cameras};
use camera_contract::ids::LensRole;
use camera_contract::three_a::{DirectiveSlot, MainsFrequency, RequestOrigin};

use super::lens_support::LensRig;
use super::standby_support::*;
use super::three_a_support::result_meta;
use super::*;
use crate::exposure_transfer::exposure_transfer;
use crate::lens_wiring::LensWiring;
use crate::three_a_own::{OwnExposureConfig, RawStatsExposure};

/// Цель, которую «рендер» предлагает на основной камере (широкой).
const MEASURED_ON_WIDE: f64 = 0.4;

#[derive(Default)]
struct CountingSignal(AtomicU32);

impl RefreshSignal for CountingSignal {
    fn signal(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Стенд тёплых камер с нашим AE и ящиком указаний.
struct OwnWarmRig {
    warm: StandbyRig,
    channel: ExposureProposalChannel,
    directives: DirectiveSlot<ExposureDirective>,
    signal: Arc<CountingSignal>,
    delivered: u32,
}

impl OwnWarmRig {
    /// Основная (широкая) стримит 4:3@60, предложение рендера принято: память цели есть.
    fn streaming_on_wide_with_a_measured_scene() -> Self {
        let config = CameraEngineConfig::default();
        let intents = camera_contract::zoom::LensIntentChannel::new();
        let channel = ExposureProposalChannel::new();
        let signal = Arc::new(CountingSignal::default());
        channel
            .refresh_signal()
            .bind(Arc::clone(&signal) as Arc<dyn RefreshSignal>);
        let directives = DirectiveSlot::new();
        let own = RawStatsExposure::new(OwnExposureConfig::default(), channel.clone(), MainsFrequency::Hz50);
        let wiring = ThreeAWiring::vendor(&config.three_a)
            .with_controllers(
                Box::new(own),
                Box::new(crate::three_a_vendor::VendorFocus::new(
                    config.three_a.focus_marker_grace_frames,
                )),
            )
            .with_exposure_directive_slot(directives.clone());
        let fake = FakeBackendControl::new();
        let mut cameras = oneplus11_visible_cameras();
        cameras.push(main_wide_camera());
        fake.set_cameras(oneplus11_visible_cameras());
        let collector = Arc::new(Collector::default());
        let listener: Arc<dyn BackendEventListener> = Arc::clone(&collector) as Arc<dyn BackendEventListener>;
        let backend = fake.backend(BackendEventSink::new(listener));
        let mut standby_config = config;
        standby_config.standby.max_cameras = 1;
        let engine = CameraEngine::with_wiring(standby_config, AppPresence::Foreground, backend, wiring)
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
        let mut own_rig = Self {
            warm: StandbyRig { lens },
            channel,
            directives,
            signal,
            delivered: 0,
        };
        // Первый результат потока (метка начала потока), затем предложение рендера по более позднему кадру.
        own_rig
            .warm
            .lens
            .rig
            .feed_result(result_meta(1, RequestOrigin::Repeating, None));
        own_rig.deliver_pushes();
        own_rig.propose(MEASURED_ON_WIDE, "2");
        own_rig.warm.lens.rig.fresh_events();
        own_rig.warm.lens.rig.fake.take_calls();
        own_rig
    }

    /// Рендер положил предложение, измеренное на камере `camera`.
    fn propose(&mut self, target: f64, camera: &str) {
        self.channel.publish(ExposureProposal {
            measured_frame: SensorTimestamp::from_nanos(i64::MAX / 2),
            measured_on: CameraTag::of(&camera_id(camera)),
            target: ExposureProduct::from_seconds_iso(target).expect("цель"),
            metered_area: MeteredArea::WholeFrame,
            convergence: ProposalConvergence::Settled,
            quality: MeasurementQuality::Measured,
        });
        self.deliver_pushes();
    }

    /// Поток камеры: каждый новый толчок — один `refresh_three_a`.
    fn deliver_pushes(&mut self) {
        let pushes = self.signal.0.load(Ordering::SeqCst);
        if pushes > self.delivered {
            self.delivered = pushes;
            self.warm.lens.rig.engine.refresh_three_a(self.warm.lens.rig.now);
            self.warm.lens.rig.pump();
        }
    }

    fn stream_start(&self) -> Option<CarriedSceneExposure> {
        self.directives.latest().expect("указание опубликовано").1.stream_start
    }

    /// Ожидаемое произведение ширика (камера «2») → ультраширика (камера «3») для цели основной.
    fn expected_on_ultra_wide(&self) -> CarriedSceneExposure {
        let catalog = oneplus11_visible_cameras();
        let find = |id: &str| {
            catalog
                .iter()
                .find(|camera| camera.id == camera_id(id))
                .expect("камера каталога")
        };
        let transfer = exposure_transfer(&find("2").capabilities, &find("3").capabilities);
        let target = ExposureProduct::from_seconds_iso(MEASURED_ON_WIDE * transfer.factor).expect("цель");
        CarriedSceneExposure::for_camera(&camera_id("3"), target)
    }
}

fn assert_same_exposure(actual: Option<CarriedSceneExposure>, expected: CarriedSceneExposure, what: &str) {
    let actual = actual.unwrap_or_else(|| panic!("{what}: ожидание не опубликовано"));
    assert_eq!(actual.camera, expected.camera, "{what}: камера");
    let difference_ev = actual.target.ev_above(expected.target).abs();
    assert!(
        difference_ev < 0.01,
        "{what}: расхождение {difference_ev} EV: {actual:?} против {expected:?}"
    );
}

#[test]
fn the_first_stream_of_a_session_has_nothing_to_carry() {
    let rig = OwnWarmRig::streaming_on_wide_with_a_measured_scene();
    assert_eq!(
        rig.stream_start(),
        None,
        "поток на широкой начался без прошлого: ожидать нечего"
    );
}

#[test]
fn a_normal_reopen_publishes_the_transferred_target_for_the_new_camera() {
    let mut rig = OwnWarmRig::streaming_on_wide_with_a_measured_scene();
    // Тёплой нет: смена объектива пойдёт обычным закрытием и открытием.
    rig.warm.lens.rig.send(CameraCommand::SwitchTo {
        role: LensRole::UltraWide,
    });
    assert_eq!(rig.warm.lens.rig.fake.open_camera(), Some(camera_id("3")));
    let expected = rig.expected_on_ultra_wide();
    assert_same_exposure(rig.stream_start(), expected, "переоткрытие");
    // Тот же перенос видит запрос нового потока: стартовая экспозиция ультраширика = ожидание.
    let (_, controls) = rig.warm.lens.rig.fake.repeating_request().expect("запрос идёт");
    let camera_core_product = match controls.exposure {
        crate::request_controls::ExposurePlan::Sensor { exposure_time, iso } => {
            exposure_time.duration().as_secs_f64() * f64::from(iso.value())
        }
        other => panic!("ожидали Sensor: {other:?}"),
    };
    assert!(
        (camera_core_product / expected.target.seconds_iso()).log2().abs() < 0.2,
        "запрос {camera_core_product} с·ISO против ожидания {} с·ISO (допуск на сетку выдержки и ISO)",
        expected.target.seconds_iso()
    );
}

#[test]
fn a_promoted_standby_publishes_the_transferred_target_before_the_first_new_frame() {
    let mut rig = OwnWarmRig::streaming_on_wide_with_a_measured_scene();
    rig.warm.wheel_at(1.0);
    rig.warm.make_standby_ready(0);
    assert_eq!(
        rig.stream_start(),
        None,
        "тёплая только греется: ожидание ещё не объявлено"
    );

    rig.warm.tap_while_warm(0.5);

    assert!(
        rig.warm.calls().contains(&"swap_standby_with_primary"),
        "обмен состоялся"
    );
    assert_eq!(rig.warm.standby().counters.promotions, 1);
    assert_same_exposure(rig.stream_start(), rig.expected_on_ultra_wide(), "обмен тёплой");
    // Новых кадров ещё нет (первый кадр новой основной не пришёл): указание на месте раньше них.
    assert_eq!(rig.warm.lens.rig.engine.status().stream.results_completed, 0);
}

#[test]
fn the_expectation_is_announced_before_the_swap_and_does_not_change_the_state_it_does_not_own() {
    let mut rig = OwnWarmRig::streaming_on_wide_with_a_measured_scene();
    let before = rig.warm.lens.rig.engine.status().three_a.auto_exposure;
    let generation_before = rig.directives.latest().expect("указание").0;

    rig.warm
        .lens
        .rig
        .engine
        .announce_expected_exposure_before_swap(&camera_id("3"));

    assert_same_exposure(rig.stream_start(), rig.expected_on_ultra_wide(), "до обмена");
    assert_ne!(
        rig.directives.latest().expect("указание").0,
        generation_before,
        "указание опубликовано"
    );
    assert_eq!(
        rig.warm.lens.rig.engine.status().three_a.auto_exposure,
        before,
        "состояние AE основной не тронуто"
    );
    // Отказ обмена снимает объявленное ожидание (основная продолжает стримить прежнюю камеру).
    rig.warm.lens.rig.engine.withdraw_expected_exposure();
    assert_eq!(rig.stream_start(), None);
}

#[test]
fn a_refused_swap_falls_back_to_reopening_and_still_ends_with_the_right_expectation() {
    let mut rig = OwnWarmRig::streaming_on_wide_with_a_measured_scene();
    rig.warm.wheel_at(1.0);
    rig.warm.make_standby_ready(0);
    // Платформа закрыла тёплую без события: обмен будет отвергнут, поедем обычным путём.
    rig.warm.lens.rig.fake.close_standby_silently(slot(0));

    rig.warm.tap_while_warm(0.5);

    assert_eq!(rig.warm.standby().counters.promotion_fallbacks, 1);
    assert_eq!(rig.warm.lens.rig.fake.open_camera(), Some(camera_id("3")));
    assert_same_exposure(rig.stream_start(), rig.expected_on_ultra_wide(), "после отказа обмена");
}

#[test]
fn a_late_proposal_from_the_old_camera_after_the_swap_does_not_move_the_new_cameras_exposure() {
    let mut rig = OwnWarmRig::streaming_on_wide_with_a_measured_scene();
    rig.warm.wheel_at(1.0);
    rig.warm.make_standby_ready(0);
    rig.warm.tap_while_warm(0.5);
    rig.warm.lens.rig.fake.take_calls();
    // Первый результат новой основной, затем запоздавшее предложение СТАРОЙ камеры с «свежей» меткой.
    rig.warm
        .lens
        .rig
        .fake
        .complete_frames(1, crate::engine::tests::lens_support::FRAME);
    rig.warm.lens.rig.pump();
    let request_before = rig.warm.lens.rig.fake.repeating_request();
    rig.propose(0.001, "2");
    assert_eq!(
        rig.warm.lens.rig.fake.repeating_request(),
        request_before,
        "цель старой камеры запрос новой не перестроила"
    );
    assert!(
        rig.warm.lens.rig.fake.take_calls().is_empty(),
        "ни одного вызова бэкенда из-за чужого предложения"
    );
}
