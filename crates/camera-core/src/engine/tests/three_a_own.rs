//! Наш AE внутри настоящего движка (fake-бэкенд): предложения через настоящий
//! `ExposureProposalChannel`, пробуждение, пересборка запроса, указание рендеру, статус.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use camera_contract::auto_exposure::{
    ExposureAuthority, ExposureDirective, ExposureProduct, ExposureProposal, ExposureProposalChannel,
    MeasurementQuality, MeteredArea, ProductReach, ProposalConvergence, RefreshSignal,
};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::{Control, ExposureBias, ExposureTime, Iso, ManualControl};
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::three_a::{
    DirectiveGeneration, DirectiveSlot, MainsFrequency, PointOfInterest, RequestOrigin, SceneFlicker,
};

use super::three_a_support::*;
use super::*;
use crate::config::ThreeAConfig;
use crate::request_controls::ExposurePlan;
use crate::three_a_own::{FlickerPolicy, OwnExposureConfig, RawStatsExposure};
use crate::three_a_status::{AdjustmentReason, AutoExposureProgress, ControlParameter};
use crate::three_a_vendor::VendorFocus;

/// Метка кадра `frame` на той же шкале, что у `result_meta`.
fn timestamp(frame: i64) -> i64 {
    1_000_000_000 + frame * 16_666_667
}

#[derive(Default)]
struct CountingSignal(AtomicU32);

impl RefreshSignal for CountingSignal {
    fn signal(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Стенд с нашим AE: настоящий ящик предложений, ящик указаний, счётчики.
struct OwnRig {
    rig: Rig,
    channel: ExposureProposalChannel,
    directives: DirectiveSlot<ExposureDirective>,
    signal: Arc<CountingSignal>,
    stale: Arc<AtomicU64>,
    /// Сколько толчков уже превращено в вызов `refresh_three_a` (как это делает поток камеры).
    delivered: u32,
    next_frame: i64,
}

impl OwnRig {
    /// Стенд с включённой защитой 50 Гц; умолчание конфига (выключена, решение 77) проверяет
    /// отдельный тест.
    fn new() -> Self {
        Self::with_exposure_config(protected_exposure_config())
    }

    fn with_exposure_config(exposure: OwnExposureConfig) -> Self {
        let config = CameraEngineConfig {
            three_a: ThreeAConfig {
                exposure,
                ..ThreeAConfig::default()
            },
            ..CameraEngineConfig::default()
        };
        let channel = ExposureProposalChannel::new();
        let signal = Arc::new(CountingSignal::default());
        channel
            .refresh_signal()
            .bind(Arc::clone(&signal) as Arc<dyn RefreshSignal>);
        let own = RawStatsExposure::new(config.three_a.exposure, channel.clone(), MainsFrequency::Hz50);
        let stale = own.stale_proposals_counter();
        let directives = DirectiveSlot::new();
        let wiring = ThreeAWiring::vendor(&config.three_a)
            .with_controllers(
                Box::new(own),
                Box::new(VendorFocus::new(config.three_a.focus_marker_grace_frames)),
            )
            .with_exposure_directive_slot(directives.clone());
        let mut rig = Rig::with_wiring(config, wiring);
        rig.start_streaming_on_wide();
        Self {
            rig,
            channel,
            directives,
            signal,
            stale,
            delivered: 0,
            next_frame: 1,
        }
    }

    /// Результат повторяющегося запроса со своей меткой; возвращает номер кадра.
    fn feed_repeating(&mut self, flicker: Option<SceneFlicker>) -> i64 {
        let frame = self.next_frame;
        self.next_frame += 1;
        let mut result = result_meta(frame, RequestOrigin::Repeating, None);
        result.three_a.scene_flicker = flicker;
        self.rig.feed_result(result);
        self.deliver_pushes();
        frame
    }

    /// Рендер положил предложение по кадру `frame`.
    fn propose(&mut self, frame: i64, target: f64, convergence: ProposalConvergence) {
        self.channel.publish(ExposureProposal {
            measured_frame: SensorTimestamp::from_nanos(timestamp(frame)),
            target: ExposureProduct::from_seconds_iso(target).expect("цель"),
            metered_area: MeteredArea::WholeFrame,
            convergence,
            quality: MeasurementQuality::Measured,
        });
        self.deliver_pushes();
    }

    /// Поток камеры: каждый новый толчок — один `refresh_three_a`.
    fn deliver_pushes(&mut self) {
        let pushes = self.signal.0.load(Ordering::SeqCst);
        if pushes > self.delivered {
            self.delivered = pushes;
            self.rig.engine.refresh_three_a(self.rig.now);
            self.rig.pump();
        }
    }

    fn sensor(&self) -> (Duration, u32) {
        match self.rig.fake.repeating_request().expect("запрос идёт").1.exposure {
            ExposurePlan::Sensor { exposure_time, iso } => (exposure_time.duration(), iso.value()),
            other => panic!("ожидали Sensor: {other:?}"),
        }
    }

    fn directive(&self) -> (DirectiveGeneration, ExposureDirective) {
        self.directives.latest().expect("указание опубликовано с самого начала")
    }

    fn progress(&self) -> Option<AutoExposureProgress> {
        self.rig.engine.status().three_a.auto_exposure
    }
}

fn manual(iso: u32, shutter_ms: u64) -> ManualControl {
    ManualControl {
        iso: Control::Manual(Iso::new(iso).expect("ISO")),
        shutter: Control::Manual(ExposureTime::new(Duration::from_millis(shutter_ms)).expect("выдержка")),
        ..ManualControl::AUTOMATIC
    }
}

#[test]
fn own_exposure_never_falls_back_to_vendor_auto_on_any_camera_of_the_fixtures() {
    let targets = [
        CameraTarget::Role(LensRole::Wide),
        CameraTarget::Role(LensRole::UltraWide),
        CameraTarget::Role(LensRole::Front),
        CameraTarget::Camera(camera_contract::fixtures::camera_id("0")),
    ];
    for target in targets {
        let channel = ExposureProposalChannel::new();
        let config = CameraEngineConfig::default();
        let wiring = ThreeAWiring::own(&config.three_a, channel, MainsFrequency::Hz50);
        let mut rig = Rig::with_wiring(config, wiring);
        assert_eq!(
            rig.send(CameraCommand::Open { target: target.clone() }),
            CommandOutcome::Accepted
        );
        assert_eq!(
            rig.send(CameraCommand::Configure { mode: mode_raw10_60() }),
            CommandOutcome::Accepted,
            "{target:?}"
        );
        assert_eq!(
            rig.send(CameraCommand::StartStreaming {
                intent: CaptureIntent::Preview
            }),
            CommandOutcome::Accepted,
            "{target:?}"
        );
        let steps = [
            CameraCommand::FocusAndExposeAt { point: upper_left() },
            CameraCommand::SetExposureBias {
                bias: ExposureBias::from_ev(1.5).expect("EV"),
            },
            CameraCommand::LockFocusAndExposureAt { point: center() },
            CameraCommand::ApplyControls {
                controls: ManualControl {
                    iso: Control::Manual(Iso::new(400).expect("ISO")),
                    ..ManualControl::AUTOMATIC
                },
            },
            CameraCommand::ApplyControls {
                controls: manual(200, 4),
            },
            CameraCommand::ApplyControls {
                controls: ManualControl::AUTOMATIC,
            },
            CameraCommand::ResetPointOfInterest,
            CameraCommand::SwitchTo {
                role: LensRole::UltraWide,
            },
        ];
        for step in steps {
            let outcome = rig.send(step.clone());
            assert!(
                matches!(outcome, CommandOutcome::Accepted | CommandOutcome::AlreadyInState),
                "{target:?}: {step:?} -> {outcome:?}"
            );
            let (_, controls) = rig.fake.repeating_request().expect("запрос идёт");
            assert!(
                matches!(controls.exposure, ExposurePlan::Sensor { .. }),
                "{target:?} после {step:?}: {:?}",
                controls.exposure
            );
        }
        let all = repeating_plans(&rig.fake.take_calls());
        assert!(!all.is_empty());
        assert!(
            all.iter()
                .all(|plan| matches!(plan.exposure, ExposurePlan::Sensor { .. })),
            "{target:?}: в журнале бэкенда встретился VendorAuto"
        );
    }
}

#[test]
fn a_new_proposal_rebuilds_the_request_exactly_once_and_repeats_do_not() {
    let mut own = OwnRig::new();
    assert_eq!(
        own.sensor(),
        (Duration::from_millis(10), 400),
        "старт из конфига: 4 с·ISO под защитой 50 Гц"
    );
    let first = own.feed_repeating(None);
    own.rig.fake.take_calls();
    own.propose(first, 0.4, ProposalConvergence::Searching);
    let calls = own.rig.fake.take_calls();
    assert_eq!(
        call_names(&calls),
        vec!["set_repeating_request"],
        "ровно одна пересборка"
    );
    assert_eq!(own.sensor(), (Duration::from_millis(4), 100));

    for _ in 0..10 {
        own.propose(first + 1, 0.4, ProposalConvergence::Searching);
    }
    assert!(
        own.rig.fake.take_calls().is_empty(),
        "одинаковые предложения запрос не трогают"
    );
}

#[test]
fn a_proposal_from_the_previous_stream_is_not_applied_and_is_counted() {
    let mut own = OwnRig::new();
    let first = own.feed_repeating(None);
    own.propose(first, 0.4, ProposalConvergence::Settled);
    assert_eq!(own.sensor(), (Duration::from_millis(4), 100));

    own.rig.send(CameraCommand::StopStreaming);
    own.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    own.rig.fake.take_calls();
    // Новый поток начался на кадре 500; рендер успел выложить предложение по кадру из старого.
    own.next_frame = 500;
    let restart_frame = own.feed_repeating(None);
    own.propose(restart_frame - 400, 0.01, ProposalConvergence::Settled);
    assert_eq!(own.stale.load(Ordering::SeqCst), 1);
    assert!(
        own.rig.fake.take_calls().is_empty(),
        "устаревшая цель запрос не перестроила"
    );
    assert_eq!(
        own.sensor(),
        (Duration::from_millis(4), 100),
        "стартуем с памяти прошлого потока"
    );

    own.propose(restart_frame + 1, 0.2, ProposalConvergence::Searching);
    assert_eq!(own.stale.load(Ordering::SeqCst), 1);
    assert_eq!(
        own.sensor(),
        (Duration::from_millis(2), 100),
        "свежее предложение принято"
    );
}

/// Конфиг с явно включённой защитой от мерцания (ручной режим).
fn protected_exposure_config() -> OwnExposureConfig {
    OwnExposureConfig {
        flicker: FlickerPolicy::AlwaysProtect,
        ..OwnExposureConfig::default()
    }
}

#[test]
fn hal_no_flicker_reports_do_not_clear_the_protection_and_never_touch_the_request() {
    let mut own = OwnRig::new();
    let first = own.feed_repeating(None);
    own.propose(first, 1.2, ProposalConvergence::Settled);
    assert_eq!(own.sensor(), (Duration::from_millis(10), 120));
    own.rig.fake.take_calls();

    // Раньше 30 «чистых» кадров снимали защиту; детектор HAL фиктивный (решение 77).
    for _ in 0..60 {
        own.feed_repeating(Some(SceneFlicker::None));
    }
    assert!(
        own.rig.fake.take_calls().is_empty(),
        "отчёты HAL запрос не перестраивают"
    );
    assert_eq!(own.sensor(), (Duration::from_millis(10), 120), "защита осталась");
}

#[test]
fn default_policy_is_off_and_a_hal_60hz_report_does_not_switch_protection_on() {
    let mut own = OwnRig::with_exposure_config(OwnExposureConfig::default());
    let first = own.feed_repeating(None);
    own.propose(first, 1.2, ProposalConvergence::Settled);
    assert_eq!(
        own.sensor(),
        (Duration::from_millis(12), 100),
        "по умолчанию выдержка свободна"
    );
    own.rig.fake.take_calls();
    for _ in 0..30 {
        own.feed_repeating(Some(SceneFlicker::Mains60Hz));
    }
    assert!(own.rig.fake.take_calls().is_empty());
    assert_eq!(own.sensor(), (Duration::from_millis(12), 100));
}

#[test]
fn planning_for_validation_and_commands_does_not_disturb_later_plans_or_counters() {
    let mut own = OwnRig::new();
    let first = own.feed_repeating(None);
    own.propose(first, 0.8, ProposalConvergence::Settled);
    let before = own.rig.fake.repeating_request().expect("запрос идёт").1;
    for ev in [1.0, -1.0, 0.0] {
        own.rig.send(CameraCommand::SetExposureBias {
            bias: ExposureBias::from_ev(ev).expect("EV"),
        });
    }
    assert_eq!(
        own.rig.fake.repeating_request().expect("запрос идёт").1,
        before,
        "после возврата компенсации план прежний"
    );
    own.rig.fake.take_calls();
    own.rig.engine.refresh_three_a(own.rig.now);
    assert!(
        own.rig.fake.take_calls().is_empty(),
        "validate/plan не оставили работы для refresh"
    );
    assert_eq!(own.stale.load(Ordering::SeqCst), 0);
}

#[test]
fn directive_carries_what_the_repeating_request_really_holds_and_publishes_only_on_change() {
    let mut own = OwnRig::new();
    let (generation_at_start, _) = own.directive();
    let first = own.feed_repeating(None);
    own.propose(first, 0.4, ProposalConvergence::Settled);
    let (shutter, iso) = own.sensor();
    let (_, directive) = own.directive();
    let applied = directive.applied.expect("запрос стоит — applied есть");
    let expected = shutter.as_secs_f64() * f64::from(iso);
    assert!((applied.product.seconds_iso() - expected).abs() < 1e-12);
    assert_eq!(applied.shortfall_ev, 0.0);
    assert_eq!(directive.authority, ExposureAuthority::Automatic);
    assert_eq!(directive.metering, PointOfInterest::WholeFrame);

    let (generation, _) = own.directive();
    assert!(generation > generation_at_start);
    for _ in 0..5 {
        own.propose(first + 1, 0.4, ProposalConvergence::Settled);
        own.rig.engine.refresh_three_a(own.rig.now);
    }
    assert_eq!(own.directive().0, generation, "на повторе поколение не растёт");
}

#[test]
fn directive_follows_tap_bias_manual_mode_and_the_end_of_the_stream() {
    let mut own = OwnRig::new();
    own.rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() });
    let tap = own.rig.last_command_id();
    match own.directive().1.metering {
        PointOfInterest::Point { request, .. } => assert_eq!(request.0, tap.0),
        PointOfInterest::WholeFrame => panic!("тап должен стать областью замера"),
    }

    own.rig.send(CameraCommand::SetExposureBias {
        bias: ExposureBias::from_ev(-0.7).expect("EV"),
    });
    assert_eq!(own.directive().1.bias.ev(), -0.7);

    own.rig.send(CameraCommand::ApplyControls {
        controls: manual(200, 4),
    });
    let (_, manual_directive) = own.directive();
    assert_eq!(manual_directive.authority, ExposureAuthority::Manual);
    let applied = manual_directive.applied.expect("ручной запрос стоит");
    assert!((applied.product.seconds_iso() - 0.8).abs() < 1e-12, "4 мс × ISO 200");
    assert_eq!(applied.shortfall_ev, 0.0);

    own.rig.send(CameraCommand::ApplyControls {
        controls: ManualControl {
            iso: Control::Manual(Iso::new(200).expect("ISO")),
            ..ManualControl::AUTOMATIC
        },
    });
    assert_eq!(own.directive().1.authority, ExposureAuthority::SemiAutomatic);

    own.rig.send(CameraCommand::StopStreaming);
    assert_eq!(own.directive().1.applied, None, "запроса нет — подтягивать нечего");
    own.rig.send(CameraCommand::Close);
    assert_eq!(
        own.directive().1,
        ExposureDirective::DEFAULT,
        "закрытие сбросило намерение, указание снова по умолчанию"
    );
}

#[test]
fn directive_reports_the_honest_shortfall_when_camera_limits_are_hit() {
    let mut own = OwnRig::new();
    let first = own.feed_repeating(None);
    own.propose(first, 1_000.0, ProposalConvergence::Searching);
    let applied = own.directive().1.applied.expect("запрос стоит");
    // Защита 50 Гц держит выдержку 10 мс: потолок 10 мс × ISO 6400 = 64 с·ISO, 1000 → 64 ≈ −3,97 EV.
    assert!(
        (applied.shortfall_ev + 3.97).abs() < 0.06,
        "честная нехватка около −4 EV, а не {}",
        applied.shortfall_ev
    );
    // Указание несёт стабильный достижимый отрезок: рендер срезает по нему цель, а не по нехватке выше.
    let ProductReach::Between { brightest, .. } = applied.reach else {
        panic!("наш AE обязан отдать достижимый отрезок: {:?}", applied.reach);
    };
    assert!(
        (brightest.seconds_iso() - 64.0).abs() < 1e-6,
        "10 мс × ISO 6400, а не {brightest:?}"
    );
    let expected = own.rig.engine.status().three_a.adjustments;
    assert!(expected.iter().any(|adjustment| {
        adjustment.parameter == ControlParameter::ExposureTarget
            && adjustment.reason == AdjustmentReason::TargetNotReached
    }));
}

#[test]
fn status_tracks_the_accepted_proposal_the_limits_and_manual_mode() {
    let mut own = OwnRig::new();
    assert_eq!(own.progress(), Some(AutoExposureProgress::AwaitingMeasurement));
    let first = own.feed_repeating(None);
    own.propose(first, 0.4, ProposalConvergence::Searching);
    assert_eq!(own.progress(), Some(AutoExposureProgress::Searching));
    own.propose(first + 1, 0.5, ProposalConvergence::Settled);
    assert_eq!(own.progress(), Some(AutoExposureProgress::Settled));
    own.propose(first + 2, 0.5, ProposalConvergence::Holding);
    assert_eq!(own.progress(), Some(AutoExposureProgress::Holding));
    own.propose(first + 3, 1_000.0, ProposalConvergence::Searching);
    assert!(matches!(own.progress(), Some(AutoExposureProgress::LimitReached { shortfall_ev }) if shortfall_ev < 0.0));
    own.rig.send(CameraCommand::ApplyControls {
        controls: manual(100, 4),
    });
    assert_eq!(own.progress(), Some(AutoExposureProgress::Manual));
}

#[test]
fn vendor_wiring_reports_no_own_exposure_state() {
    let mut rig = streaming_rig();
    assert_eq!(rig.engine.status().three_a.auto_exposure, None);
    rig.send(CameraCommand::SetExposureBias {
        bias: ExposureBias::from_ev(1.0).expect("EV"),
    });
    assert_eq!(rig.engine.status().three_a.auto_exposure, None);
}

#[test]
fn lock_wins_over_the_limit_in_the_status() {
    let mut own = OwnRig::new();
    let first = own.feed_repeating(None);
    own.rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    own.propose(first, 1_000.0, ProposalConvergence::Holding);
    assert_eq!(
        own.progress(),
        Some(AutoExposureProgress::Holding),
        "блокировка важнее предела"
    );
    // Без блокировки та же цель — предел.
    own.rig.send(CameraCommand::ResetPointOfInterest);
    own.propose(first + 1, 1_001.0, ProposalConvergence::Holding);
    assert!(matches!(
        own.progress(),
        Some(AutoExposureProgress::LimitReached { .. })
    ));
}

#[test]
fn target_jitter_at_a_rounding_boundary_with_the_same_shortfall_gives_one_adjustment_event() {
    let mut own = OwnRig::with_exposure_config(OwnExposureConfig {
        flicker: FlickerPolicy::NeverProtect,
        ..OwnExposureConfig::default()
    });
    let first = own.feed_repeating(None);
    own.rig.fresh_events();
    // Потолок 16,667 мс × ISO 6400 ≈ 2^6,74; цели 2^7,63 и 2^7,67 дают нехватку −0,89 и −0,93 EV,
    // обе округляются в −0,9, хотя log2 самой цели лежит по обе стороны границы 7,65.
    for (index, log2_target) in [7.63_f64, 7.67, 7.63, 7.67].into_iter().enumerate() {
        own.propose(first + index as i64, log2_target.exp2(), ProposalConvergence::Searching);
    }
    let events = adjustment_events(&own.rig.fresh_events());
    assert_eq!(events.len(), 1, "нехватка не менялась — событие одно: {events:?}");
    let [adjustment] = events[0].as_slice() else {
        panic!("{events:?}")
    };
    assert_eq!(adjustment.requested, 0.0);
    assert!((adjustment.applied + 0.9).abs() < 1e-6);
}
