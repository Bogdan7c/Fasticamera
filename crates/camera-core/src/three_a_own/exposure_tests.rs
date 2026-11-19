//! Исполнитель `RawStatsExposure` как чёрный ящик по трейту: что он планирует после
//! предложений, мерцания и смены потока. Движок вокруг — в `engine/tests/three_a_own.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use camera_contract::auto_exposure::{
    ExposureProduct, ExposureProposal, ExposureProposalChannel, MeasurementQuality, MeteredArea, ProposalConvergence,
    RefreshSignal,
};
use camera_contract::capabilities::{CameraCapabilities, ManualSensorRanges};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::{CaptureIntent, Control, ExposureBias, ExposureTime, Iso, ManualControl};
use camera_contract::fixtures::main_wide_camera;
use camera_contract::stream::TargetFrameRate;
use camera_contract::three_a::{MainsFrequency, RequestOrigin, SceneFlicker, ThreeAResultState};

use super::config::{FlickerPolicy, OwnExposureConfig};
use super::exposure::RawStatsExposure;
use crate::command::UnsupportedThreeA;
use crate::request_controls::ExposurePlan;
use crate::three_a_control::{ExposureController, ExposureDecision, PlanRefresh, ThreeAContext, ThreeAIntent};
use crate::three_a_status::{AdjustmentReason, AutoExposureProgress, ControlParameter};
use crate::three_a_vendor::VendorExposure;

/// Считает толчки, которые исполнитель отправил движку.
#[derive(Default)]
struct CountingSignal(AtomicU32);

impl RefreshSignal for CountingSignal {
    fn signal(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    exposure: RawStatsExposure,
    channel: ExposureProposalChannel,
    signal: Arc<CountingSignal>,
    stale: Arc<AtomicU64>,
    capabilities: CameraCapabilities,
}

impl Fixture {
    /// Стенд с включённой защитой 50 Гц: большинство тестов проверяют кратность 10 мс.
    /// Умолчание конфига (защита выключена, решение 77) проверяют отдельные тесты.
    fn new() -> Self {
        Self::with_config(protected_config())
    }

    fn with_config(config: OwnExposureConfig) -> Self {
        Self::with_config_and_mains(config, MainsFrequency::Hz50)
    }

    fn with_config_and_mains(config: OwnExposureConfig, mains: MainsFrequency) -> Self {
        let channel = ExposureProposalChannel::new();
        let signal = Arc::new(CountingSignal::default());
        channel
            .refresh_signal()
            .bind(Arc::clone(&signal) as Arc<dyn RefreshSignal>);
        let exposure = RawStatsExposure::new(config, channel.clone(), mains);
        Self {
            stale: exposure.stale_proposals_counter(),
            exposure,
            channel,
            signal,
            capabilities: main_wide_camera().capabilities,
        }
    }

    fn plan_for(&mut self, controls: ManualControl) -> Result<ExposureDecision, UnsupportedThreeA> {
        let intent = ThreeAIntent {
            controls,
            ..ThreeAIntent::AUTOMATIC
        };
        let context = ThreeAContext {
            intent: &intent,
            capture_intent: CaptureIntent::Preview,
            capabilities: &self.capabilities,
            stream_frame_duration: TargetFrameRate::new(60).expect("60 к/с").frame_duration(),
        };
        self.exposure.plan(&context)
    }

    fn plan(&mut self) -> ExposureDecision {
        self.plan_for(ManualControl::AUTOMATIC).expect("авто-план строится")
    }

    /// Произведение и выдержка автоплана.
    fn planned(&mut self) -> (Duration, u32) {
        sensor_of(&self.plan().plan)
    }

    fn publish(&self, measured_ns: i64, target: f64, convergence: ProposalConvergence) {
        self.channel.publish(proposal(measured_ns, target, convergence));
    }

    fn pushes(&self) -> u32 {
        self.signal.0.load(Ordering::SeqCst)
    }
}

/// Конфиг с явно включённой защитой от мерцания (ручной режим).
fn protected_config() -> OwnExposureConfig {
    OwnExposureConfig {
        flicker: FlickerPolicy::AlwaysProtect,
        ..OwnExposureConfig::default()
    }
}

fn sensor_of(plan: &ExposurePlan) -> (Duration, u32) {
    match plan {
        ExposurePlan::Sensor { exposure_time, iso } => (exposure_time.duration(), iso.value()),
        other => panic!("ожидали Sensor, получили {other:?}"),
    }
}

fn product_of(plan: &ExposurePlan) -> f64 {
    let (shutter, iso) = sensor_of(plan);
    shutter.as_secs_f64() * f64::from(iso)
}

fn proposal(measured_ns: i64, target: f64, convergence: ProposalConvergence) -> ExposureProposal {
    ExposureProposal {
        measured_frame: SensorTimestamp::from_nanos(measured_ns),
        target: ExposureProduct::from_seconds_iso(target).expect("цель"),
        metered_area: MeteredArea::WholeFrame,
        convergence,
        quality: MeasurementQuality::Measured,
    }
}

fn result(timestamp_ns: i64, flicker: Option<SceneFlicker>, origin: RequestOrigin) -> CaptureResultMeta {
    CaptureResultMeta {
        frame_number: FrameNumber(timestamp_ns / 16_000_000),
        sensor_timestamp: Some(SensorTimestamp::from_nanos(timestamp_ns)),
        exposure_time: Some(Duration::from_millis(8)),
        sensitivity_iso: Some(100),
        reported_frame_duration: None,
        rolling_shutter_skew: None,
        three_a: ThreeAResultState {
            scene_flicker: flicker,
            ..ThreeAResultState::unreported(origin)
        },
    }
}

fn repeating(timestamp_ns: i64) -> CaptureResultMeta {
    result(timestamp_ns, None, RequestOrigin::Repeating)
}

fn near(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual / expected - 1.0).abs() < 0.01,
        "{what}: {actual} вместо {expected}"
    );
}

#[test]
fn before_any_measurement_the_plan_starts_from_the_configured_seed() {
    let mut fixture = Fixture::new();
    // 4,0 с·ISO, защита 50 Гц: желаемые 40 мс режутся до кадра 16,7 мс и дальше до 10 мс.
    assert_eq!(fixture.planned(), (Duration::from_millis(10), 400));
    assert_eq!(
        fixture.exposure.progress(),
        Some(AutoExposureProgress::AwaitingMeasurement)
    );
}

#[test]
fn an_accepted_proposal_drives_the_plan_and_ev_bias_scales_the_product() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 0.4, ProposalConvergence::Settled);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Changed);
    assert_eq!(fixture.planned(), (Duration::from_millis(4), 100));

    let plus_two = ManualControl {
        exposure_bias: ExposureBias::from_ev(2.0).expect("EV"),
        ..ManualControl::AUTOMATIC
    };
    let plan = fixture.plan_for(plus_two).expect("план").plan;
    near(product_of(&plan), 1.6, "+2 EV — произведение ×4");
    let minus_two = ManualControl {
        exposure_bias: ExposureBias::from_ev(-2.0).expect("EV"),
        ..ManualControl::AUTOMATIC
    };
    let plan = fixture.plan_for(minus_two).expect("план").plan;
    near(product_of(&plan), 0.1, "−2 EV — произведение ×¼");
    // Компенсация непрерывная: 0,3 EV не квантуется шагом 1/6.
    let fractional = ManualControl {
        exposure_bias: ExposureBias::from_ev(0.3).expect("EV"),
        ..ManualControl::AUTOMATIC
    };
    let plan = fixture.plan_for(fractional).expect("план").plan;
    near(product_of(&plan), 0.4 * 0.3_f64.exp2(), "дробная компенсация");
}

#[test]
fn planning_is_pure_repeated_plans_change_nothing() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 0.4, ProposalConvergence::Searching);
    fixture.exposure.refresh();
    let pushes_before = fixture.pushes();
    let first = fixture.plan();
    for _ in 0..5 {
        assert_eq!(fixture.plan(), first);
    }
    assert_eq!(
        fixture.exposure.refresh(),
        PlanRefresh::Unchanged,
        "планы не оставили работы refresh"
    );
    assert_eq!(fixture.stale.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.pushes(), pushes_before, "и толчков себе не слали");
    assert_eq!(fixture.exposure.progress(), Some(AutoExposureProgress::Searching));
}

#[test]
fn identical_and_tiny_proposals_do_not_rebuild_but_real_shifts_do() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 0.4, ProposalConvergence::Searching);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Changed);
    for index in 0..5 {
        fixture.publish(3_000 + index, 0.4, ProposalConvergence::Searching);
        assert_eq!(
            fixture.exposure.refresh(),
            PlanRefresh::Unchanged,
            "та же цель — пересборки нет"
        );
    }
    // 0,5 % ≈ 0,007 EV < порога 1/48 EV; накопленный дрейф сравнивается с планом, а не с прошлым.
    fixture.publish(4_000, 0.4 * 1.005, ProposalConvergence::Searching);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
    fixture.publish(5_000, 0.4 * 1.05, ProposalConvergence::Settled);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Changed, "0,07 EV > порога");
    assert_eq!(fixture.exposure.progress(), Some(AutoExposureProgress::Settled));
}

#[test]
fn holding_proposals_with_the_same_target_keep_the_plan_and_report_holding() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 0.4, ProposalConvergence::Searching);
    fixture.exposure.refresh();
    fixture.publish(3_000, 0.4, ProposalConvergence::Holding);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(fixture.exposure.progress(), Some(AutoExposureProgress::Holding));
    fixture.publish(4_000, 0.4, ProposalConvergence::Holding);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
}

#[test]
fn proposal_measured_before_the_new_stream_started_is_rejected_and_counted() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(5_000));
    let before = fixture.planned();
    fixture.publish(4_000, 0.05, ProposalConvergence::Settled);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(fixture.stale.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.planned(), before, "старая цель в план не попала");
    assert_eq!(
        fixture.exposure.progress(),
        Some(AutoExposureProgress::AwaitingMeasurement)
    );
    // Кадр ровно первого результата — ещё новый поток.
    fixture.publish(5_000, 0.05, ProposalConvergence::Settled);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Changed);
}

#[test]
fn proposal_that_beats_the_first_result_is_held_and_judged_when_the_stream_start_is_known() {
    let mut fixture = Fixture::new();
    // Честное предложение нового потока пришло раньше результата: не применяем и не теряем.
    fixture.publish(6_000, 0.4, ProposalConvergence::Settled);
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(fixture.stale.load(Ordering::SeqCst), 0);
    let pushes_before = fixture.pushes();
    fixture.exposure.observe_result(&repeating(5_000));
    assert_eq!(
        fixture.pushes(),
        pushes_before + 1,
        "метка появилась — исполнитель толкает движок сам"
    );
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Changed);
    assert_eq!(fixture.planned(), (Duration::from_millis(4), 100));

    // Предложение прошлого потока, пришедшее раньше результата, — устаревшее.
    let mut stale_case = Fixture::new();
    stale_case.publish(4_000, 0.4, ProposalConvergence::Settled);
    stale_case.exposure.refresh();
    stale_case.exposure.observe_result(&repeating(5_000));
    assert_eq!(stale_case.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(stale_case.stale.load(Ordering::SeqCst), 1);
}

#[test]
fn burst_and_trigger_results_do_not_start_the_stream_or_feed_the_detector() {
    let mut fixture = Fixture::new();
    let burst = result(
        9_000,
        Some(SceneFlicker::None),
        RequestOrigin::Burst {
            burst: camera_contract::three_a::BurstId(1),
            index: 0,
        },
    );
    fixture.publish(1_000, 0.4, ProposalConvergence::Settled);
    let pushes_before = fixture.pushes();
    fixture.exposure.observe_result(&burst);
    assert_eq!(
        fixture.exposure.refresh(),
        PlanRefresh::Unchanged,
        "метки потока ещё нет"
    );
    assert_eq!(fixture.stale.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.pushes(), pushes_before, "кадр серии метку потока не ставит");
}

/// Тусклая сцена: цель 1,2 с·ISO (12 мс × ISO 100 без защиты).
fn settle_dim_scene(fixture: &mut Fixture) {
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 1.2, ProposalConvergence::Settled);
    fixture.exposure.refresh();
}

#[test]
fn default_config_leaves_the_shutter_free_up_to_the_frame_limit() {
    let mut fixture = Fixture::with_config(OwnExposureConfig::default());
    assert_eq!(OwnExposureConfig::default().flicker, FlickerPolicy::NeverProtect);
    settle_dim_scene(&mut fixture);
    assert_eq!(
        fixture.planned(),
        (Duration::from_millis(12), 100),
        "без защиты выдержка свободна"
    );
    fixture.publish(3_000, 100.0, ProposalConvergence::Searching);
    fixture.exposure.observe_result(&repeating(3_500));
    fixture.exposure.refresh();
    assert!(
        fixture.planned().0 > Duration::from_millis(16),
        "яркая цель упирается в предел кадра 16,7 мс, а не в кратность 10 мс"
    );
}

#[test]
fn always_protect_at_50hz_makes_the_shutter_a_multiple_of_10ms_and_iso_covers_the_rest() {
    let mut fixture = Fixture::with_config(protected_config());
    settle_dim_scene(&mut fixture);
    assert_eq!(fixture.planned(), (Duration::from_millis(10), 120));
}

#[test]
fn always_protect_at_60hz_makes_the_shutter_a_multiple_of_8_33ms() {
    let mut fixture = Fixture::with_config_and_mains(protected_config(), MainsFrequency::Hz60);
    settle_dim_scene(&mut fixture);
    assert_eq!(fixture.planned(), (Duration::from_nanos(8_333_333), 144));
}

#[test]
fn hal_no_flicker_reports_never_clear_always_protect() {
    let mut fixture = Fixture::with_config(protected_config());
    settle_dim_scene(&mut fixture);
    let pushes_before = fixture.pushes();
    for frame in 0..100_i64 {
        fixture.exposure.observe_result(&result(
            3_000 + frame,
            Some(SceneFlicker::None),
            RequestOrigin::Repeating,
        ));
    }
    assert_eq!(fixture.pushes(), pushes_before, "отчёт HAL не толкает движок");
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(fixture.planned(), (Duration::from_millis(10), 120), "защита осталась");
}

#[test]
fn hal_60hz_reports_never_enable_protection_nor_change_the_period() {
    let mut off = Fixture::with_config(OwnExposureConfig::default());
    settle_dim_scene(&mut off);
    let pushes_before = off.pushes();
    for frame in 0..100_i64 {
        off.exposure.observe_result(&result(
            4_000 + frame,
            Some(SceneFlicker::Mains60Hz),
            RequestOrigin::Repeating,
        ));
    }
    assert_eq!(off.pushes(), pushes_before);
    assert_eq!(off.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(
        off.planned(),
        (Duration::from_millis(12), 100),
        "NeverProtect остаётся без защиты"
    );

    let mut on = Fixture::with_config(protected_config());
    settle_dim_scene(&mut on);
    on.exposure
        .observe_result(&result(4_000, Some(SceneFlicker::Mains60Hz), RequestOrigin::Repeating));
    assert_eq!(on.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(
        on.planned(),
        (Duration::from_millis(10), 120),
        "период остаётся из конфига, 50 Гц"
    );
}

#[test]
fn new_stream_remembers_the_last_target_and_keeps_the_manual_protection() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 1.2, ProposalConvergence::Settled);
    fixture.exposure.refresh();

    fixture.exposure.reset_for_new_stream();
    assert_eq!(
        fixture.exposure.progress(),
        Some(AutoExposureProgress::AwaitingMeasurement)
    );
    // Старт нового потока — прежняя цель 1,2 (а не 4,0 из конфига) и защита снова включена.
    assert_eq!(fixture.planned(), (Duration::from_millis(10), 120));
    // Ящик хранит старое предложение, но второй раз оно не читается.
    fixture.exposure.observe_result(&repeating(10_000));
    assert_eq!(fixture.exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(fixture.stale.load(Ordering::SeqCst), 0);
}

#[test]
fn without_manual_sensor_the_executor_refuses_so_the_engine_can_degrade() {
    let mut fixture = Fixture::new();
    fixture.capabilities.manual_sensor = ManualSensorRanges::NotSupported;
    assert_eq!(
        fixture.plan_for(ManualControl::AUTOMATIC),
        Err(UnsupportedThreeA::ManualSensor)
    );
}

#[test]
fn fully_manual_plan_is_exactly_the_vendor_one() {
    let mut fixture = Fixture::new();
    let controls = ManualControl {
        iso: Control::Manual(Iso::new(50).expect("ISO")),
        shutter: Control::Manual(ExposureTime::new(Duration::from_millis(40)).expect("выдержка")),
        exposure_bias: ExposureBias::from_ev(1.0).expect("EV"),
        ..ManualControl::AUTOMATIC
    };
    let own = fixture.plan_for(controls).expect("план");
    let intent = ThreeAIntent {
        controls,
        ..ThreeAIntent::AUTOMATIC
    };
    let vendor = VendorExposure
        .plan(&ThreeAContext {
            intent: &intent,
            capture_intent: CaptureIntent::Preview,
            capabilities: &fixture.capabilities,
            stream_frame_duration: TargetFrameRate::new(60).expect("60 к/с").frame_duration(),
        })
        .expect("вендорский план");
    assert_eq!(
        own, vendor,
        "те же значения и те же поправки (ISO 50 → 100, 40 мс → 16,7 мс, EV проигнорирован)"
    );
    assert!(own.adjustments.len() >= 3);
}

#[test]
fn semi_automatic_with_manual_iso_lets_the_shutter_compensate() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 0.8, ProposalConvergence::Settled);
    fixture.exposure.refresh();
    let controls = ManualControl {
        iso: Control::Manual(Iso::new(200).expect("ISO")),
        ..ManualControl::AUTOMATIC
    };
    let decision = fixture.plan_for(controls).expect("полуавтомат");
    // 0,8 / 200 = 4 мс: короче периода, защита не мешает.
    assert_eq!(sensor_of(&decision.plan), (Duration::from_millis(4), 200));
    assert!(decision.adjustments.is_empty());

    // Нужно 4 с·ISO при ISO 100: желаемая выдержка 40 мс, защита и кадр дают лишь 10 мс.
    fixture.publish(3_000, 4.0, ProposalConvergence::Settled);
    fixture.exposure.refresh();
    let controls = ManualControl {
        iso: Control::Manual(Iso::new(100).expect("ISO")),
        ..ManualControl::AUTOMATIC
    };
    let decision = fixture.plan_for(controls).expect("полуавтомат");
    assert_eq!(sensor_of(&decision.plan), (Duration::from_millis(10), 100));
    let [adjustment] = decision.adjustments.as_slice() else {
        panic!("нужна ровно одна поправка: {:?}", decision.adjustments);
    };
    assert_eq!(adjustment.parameter, ControlParameter::ExposureTarget);
    assert_eq!(adjustment.reason, AdjustmentReason::TargetNotReached);
    assert_eq!(adjustment.requested, 0.0, "цель — точка отсчёта");
    assert!(
        (adjustment.applied - adjustment.requested + 2.0).abs() < 1e-6 && adjustment.applied < 0.0,
        "набрано 1,0 из 4,0 с·ISO — нехватка ровно −2 EV"
    );
}

#[test]
fn semi_automatic_with_manual_shutter_lets_iso_compensate_and_clamps_the_manual_value() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 4.0, ProposalConvergence::Settled);
    fixture.exposure.refresh();
    let controls = ManualControl {
        shutter: Control::Manual(ExposureTime::new(Duration::from_millis(4)).expect("выдержка")),
        ..ManualControl::AUTOMATIC
    };
    let decision = fixture.plan_for(controls).expect("полуавтомат");
    assert_eq!(sensor_of(&decision.plan), (Duration::from_millis(4), 1_000));
    assert!(decision.adjustments.is_empty());

    // Ручная выдержка длиннее кадра получает те же поправки, что у вендорского плана.
    let long = ManualControl {
        shutter: Control::Manual(ExposureTime::new(Duration::from_millis(50)).expect("выдержка")),
        ..ManualControl::AUTOMATIC
    };
    let decision = fixture.plan_for(long).expect("полуавтомат");
    assert_eq!(sensor_of(&decision.plan).0, Duration::from_nanos(16_666_667));
    assert!(
        decision
            .adjustments
            .iter()
            .any(|adjustment| adjustment.reason == AdjustmentReason::LimitedByFrameDuration)
    );
}

#[test]
fn camera_limits_produce_an_honest_target_not_reached_adjustment() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    // Слишком темно: 1000 с·ISO при максимуме 16,67 мс × 6400 = 106,7.
    fixture.publish(2_000, 1_000.0, ProposalConvergence::Searching);
    fixture.exposure.refresh();
    let dark = fixture.plan();
    assert_eq!(sensor_of(&dark.plan).1, 6_400);
    let [adjustment] = dark.adjustments.as_slice() else {
        panic!("нужна одна поправка: {:?}", dark.adjustments);
    };
    assert_eq!(adjustment.reason, AdjustmentReason::TargetNotReached);
    assert!(adjustment.applied < adjustment.requested, "темнее нужного");
    // Слишком светло: 1e-7 с·ISO, короче 10 мкс × ISO 100 = 1e-3 не набрать.
    fixture.publish(3_000, 1e-7, ProposalConvergence::Searching);
    fixture.exposure.refresh();
    let bright = fixture.plan();
    assert_eq!(sensor_of(&bright.plan), (Duration::from_micros(10), 100));
    let [adjustment] = bright.adjustments.as_slice() else {
        panic!("нужна одна поправка: {:?}", bright.adjustments);
    };
    assert!(adjustment.applied > adjustment.requested, "светлее нужного");
}

#[test]
fn quantization_noise_inside_the_tolerance_is_not_reported() {
    let mut fixture = Fixture::new();
    fixture.exposure.observe_result(&repeating(1_000));
    fixture.publish(2_000, 0.123_456, ProposalConvergence::Settled);
    fixture.exposure.refresh();
    assert!(fixture.plan().adjustments.is_empty());
}
