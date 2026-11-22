//! Контроллер цели AE: холодный старт, сглаживание, мёртвая зона видео, блокировка, ручной режим,
//! чужая область, частота публикаций, подтяжка ISP.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::auto_exposure::{AppliedExposure, MeteredArea, ProductReach};
use camera_contract::control::ExposureBias;
use camera_contract::three_a::{MeteringRegion, PointOfInterest, PointRequestId, SensorPoint};

use super::*;
use crate::executor::FrameTag;
use crate::exposure::measurement::{ExposureMeasurement, ExposureMeasurementError};
use crate::exposure::test_support::{product, test_camera};

/// Кадр 60 к/с в наносекундах.
const FRAME_NANOS: i64 = 16_666_667;

pub(super) fn ts(frame: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(frame * FRAME_NANOS)
}

pub(super) fn controller() -> AutoExposureController {
    AutoExposureController::new(ExposureControllerConfig::default()).unwrap()
}

pub(super) fn directive() -> ExposureDirective {
    ExposureDirective::DEFAULT
}

pub(super) fn video() -> ExposureDirective {
    ExposureDirective {
        profile: ThreeAProfile::Video,
        ..directive()
    }
}

fn tap(request: u64) -> ExposureDirective {
    let region = MeteringRegion::new(SensorPoint::CENTER, 0.2).unwrap();
    ExposureDirective {
        metering: PointOfInterest::Point {
            region,
            request: PointRequestId(request),
        },
        ..directive()
    }
}

/// Готовый замер: сцена просит `required` с·ISO, снят на кадре `frame`.
pub(super) fn measured(
    frame: i64,
    area: MeteredArea,
    required: f64,
    quality: MeasurementQuality,
) -> CompletedExposureMeasurement {
    CompletedExposureMeasurement {
        frame: FrameTag {
            stream_generation: 1,
            sequence: frame as u64,
            sensor_timestamp_nanos: frame * FRAME_NANOS,
        },
        area,
        frame_product: product(0.01),
        outcome: Ok(ExposureMeasurement {
            required_product: product(required),
            quality,
            scene_log2_luminance: -3.7,
            clipped_fraction: 0.0,
            dark_fraction: 0.0,
            used_weight: 1.0,
            protection_ev: 0.0,
            mean_luminance: 0.075,
            mean_green_level: 0.075,
        }),
    }
}

pub(super) fn whole_frame_measurement(frame: i64, required: f64) -> CompletedExposureMeasurement {
    measured(frame, MeteredArea::WholeFrame, required, MeasurementQuality::Measured)
}

/// Контроллер, у которого цель стоит на `required` (холодный старт на кадре 0).
pub(super) fn started_at(directive: ExposureDirective, required: f64) -> AutoExposureController {
    let mut controller = controller();
    controller.set_directive(directive);
    controller.begin_stream();
    assert_eq!(
        controller.ingest(&whole_frame_measurement(0, required)),
        IngestOutcome::Accepted
    );
    controller.advance(ts(0));
    controller
}

/// Прокручивает кадры `from..to` (шаг 1), не принимая новых замеров.
pub(super) fn run_frames(controller: &mut AutoExposureController, from: i64, to: i64) {
    for frame in from..to {
        controller.advance(ts(frame));
    }
}

/// `required` выше текущей цели на `ev` ступеней.
pub(super) fn above(base: f64, ev: f64) -> f64 {
    base * ev.exp2()
}

#[test]
fn first_estimate_sets_the_target_at_once_without_smoothing() {
    let mut controller = controller();
    controller.begin_stream();
    assert!(controller.target_log2().is_none());
    assert!(
        controller.proposal(ts(0), test_camera()).is_none(),
        "цели нет — предложения нет"
    );
    controller.ingest(&whole_frame_measurement(0, 0.05));
    // Сразу после приёма и после шага времени цель равна оценке: никакого разгона от нуля.
    assert!((controller.target_log2().unwrap() - 0.05_f64.log2()).abs() < 1e-12);
    controller.advance(ts(0));
    controller.advance(ts(1));
    assert!((controller.target_log2().unwrap() - 0.05_f64.log2()).abs() < 1e-12);
    let proposal = controller.proposal(ts(1), test_camera()).unwrap();
    assert!((proposal.target.seconds_iso() - 0.05).abs() < 1e-9);
    assert_eq!(proposal.convergence, ProposalConvergence::Settled);
    assert_eq!(proposal.metered_area, MeteredArea::WholeFrame);
}

#[test]
fn target_follows_a_changed_scene_exponentially_and_never_overshoots() {
    let mut controller = started_at(directive(), 0.01);
    controller.ingest(&whole_frame_measurement(1, above(0.01, 2.0)));
    let goal = above(0.01, 2.0).log2();
    let start = 0.01_f64.log2();
    // Фото: τ = 0,15 с. За 9 кадров (0,15 с) цель проходит 1 − e^{-1} ≈ 63 % пути.
    run_frames(&mut controller, 1, 10);
    let progress = (controller.target_log2().unwrap() - start) / (goal - start);
    assert!((progress - 0.63).abs() < 0.03, "прошло {progress}");
    let mut previous = controller.target_log2().unwrap();
    for frame in 10..120 {
        controller.advance(ts(frame));
        let now = controller.target_log2().unwrap();
        assert!(
            now >= previous - 1e-12 && now <= goal + 1e-12,
            "кадр {frame}: {now} за пределами"
        );
        previous = now;
    }
    assert!((previous - goal).abs() < 0.01);
}

#[test]
fn video_profile_is_slower_than_photo() {
    let mut photo = started_at(directive(), 0.01);
    let mut slow = started_at(video(), 0.01);
    for controller in [&mut photo, &mut slow] {
        controller.ingest(&whole_frame_measurement(1, above(0.01, 2.0)));
        run_frames(controller, 1, 31);
    }
    let travelled = |controller: &AutoExposureController| controller.target_log2().unwrap() - 0.01_f64.log2();
    assert!(travelled(&photo) > 1.9, "фото уже дошло: {}", travelled(&photo));
    assert!(travelled(&slow) < 1.3, "видео ещё в пути: {}", travelled(&slow));
}

#[test]
fn video_dead_band_ignores_small_errors_and_hysteresis_runs_until_stop_threshold() {
    let mut controller = started_at(video(), 0.01);
    let base = controller.target_log2().unwrap();
    // Ошибка 0,2 EV меньше порога начала 0,25: цель не шевелится вовсе.
    controller.ingest(&whole_frame_measurement(1, above(0.01, 0.2)));
    run_frames(&mut controller, 1, 200);
    assert_eq!(controller.target_log2().unwrap(), base);
    assert_eq!(
        controller.proposal(ts(200), test_camera()).unwrap().convergence,
        ProposalConvergence::Settled,
        "покой внутри мёртвой зоны — это Settled"
    );
    // Ошибка 0,3 EV запускает движение; оно идёт и через 0,2 EV (ниже порога начала!) и останавливается у 0,05.
    controller.ingest(&whole_frame_measurement(201, above(0.01, 0.3)));
    run_frames(&mut controller, 201, 210);
    let mid_error = 0.3 - (controller.target_log2().unwrap() - base);
    assert!(
        mid_error < 0.25 && mid_error > 0.05,
        "движение должно продолжаться ниже порога начала: {mid_error}"
    );
    let before = controller.target_log2().unwrap();
    run_frames(&mut controller, 210, 210 + 20);
    assert!(
        controller.target_log2().unwrap() > before,
        "гистерезис: ещё идём к цели"
    );
    run_frames(&mut controller, 230, 1500);
    let final_error = 0.3 - (controller.target_log2().unwrap() - base);
    assert!(
        final_error < 0.05 && final_error > 0.03,
        "остановка у порога stop: {final_error}"
    );
    let resting = controller.target_log2().unwrap();
    run_frames(&mut controller, 1500, 1600);
    assert_eq!(controller.target_log2().unwrap(), resting, "после остановки цель стоит");
}

#[test]
fn photo_profile_has_no_dead_band() {
    let mut controller = started_at(directive(), 0.01);
    let base = controller.target_log2().unwrap();
    controller.ingest(&whole_frame_measurement(1, above(0.01, 0.2)));
    run_frames(&mut controller, 1, 100);
    assert!((controller.target_log2().unwrap() - base - 0.2).abs() < 0.01);
}

#[test]
fn measurement_for_another_area_is_rejected_and_counted() {
    let mut controller = controller();
    controller.set_directive(tap(3));
    controller.begin_stream();
    // Матричный замер при указании «точка 3» — чужая область.
    assert_eq!(
        controller.ingest(&whole_frame_measurement(0, 0.01)),
        IngestOutcome::RejectedForeignArea
    );
    assert_eq!(controller.counters().rejected_foreign_area, 1);
    assert!(controller.target_log2().is_none(), "чужой замер не ставит цель");
    let own = measured(
        1,
        MeteredArea::Point(PointRequestId(3)),
        0.02,
        MeasurementQuality::Measured,
    );
    assert_eq!(controller.ingest(&own), IngestOutcome::Accepted);
    let stale_tap = measured(
        2,
        MeteredArea::Point(PointRequestId(2)),
        0.5,
        MeasurementQuality::Measured,
    );
    assert_eq!(controller.ingest(&stale_tap), IngestOutcome::RejectedForeignArea);
    assert!(
        (controller.target_log2().unwrap() - 0.02_f64.log2()).abs() < 1e-12,
        "цель осталась прежней"
    );
    assert_eq!(controller.counters().rejected_foreign_area, 2);
    assert_eq!(controller.counters().accepted, 1);
}

#[test]
fn changing_the_area_drops_the_old_scene_estimate_but_keeps_the_target() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(tap(1));
    assert!(
        controller.proposal(ts(5), test_camera()).is_none(),
        "оценки новой области ещё нет"
    );
    assert!(controller.target_log2().is_some());
    controller.ingest(&measured(
        6,
        MeteredArea::Point(PointRequestId(1)),
        0.04,
        MeasurementQuality::Measured,
    ));
    let proposal = controller.proposal(ts(6), test_camera()).unwrap();
    assert_eq!(proposal.metered_area, MeteredArea::Point(PointRequestId(1)));
}

#[test]
fn clipped_and_dark_measurements_are_searching_never_settled() {
    let mut controller = controller();
    controller.begin_stream();
    controller.ingest(&measured(
        0,
        MeteredArea::WholeFrame,
        0.01,
        MeasurementQuality::ClippedHighlights,
    ));
    controller.advance(ts(0));
    let proposal = controller.proposal(ts(0), test_camera()).unwrap();
    assert_eq!(proposal.convergence, ProposalConvergence::Searching);
    assert_eq!(proposal.quality, MeasurementQuality::ClippedHighlights);
}

#[test]
fn lock_freezes_the_target_after_settling_and_unlock_resumes_following() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        ..directive()
    });
    controller.ingest(&whole_frame_measurement(1, 0.01));
    controller.advance(ts(1));
    assert!(controller.is_holding());
    assert_eq!(
        controller.proposal(ts(1), test_camera()).unwrap().convergence,
        ProposalConvergence::Holding
    );
    let frozen = controller.target_log2().unwrap();
    // Сцена ярче на 2 EV: замер игнорируется, цель стоит.
    assert_eq!(
        controller.ingest(&whole_frame_measurement(2, above(0.01, 2.0))),
        IngestOutcome::IgnoredHolding
    );
    run_frames(&mut controller, 2, 200);
    assert_eq!(controller.target_log2().unwrap(), frozen);
    assert_eq!(controller.counters().ignored_holding, 1);
    // Снятие блокировки: снова следуем.
    controller.set_directive(directive());
    assert!(!controller.is_holding());
    controller.ingest(&whole_frame_measurement(200, above(0.01, 2.0)));
    run_frames(&mut controller, 200, 400);
    assert!((controller.target_log2().unwrap() - frozen - 2.0).abs() < 0.01);
}

#[test]
fn lock_catches_up_with_the_scene_before_freezing() {
    let mut controller = started_at(directive(), 0.01);
    controller.ingest(&whole_frame_measurement(1, above(0.01, 2.0)));
    controller.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        ..directive()
    });
    // Блокировка не замораживает цель на полпути: ждёт Settled (в профиле Photo скорость быстрая).
    run_frames(&mut controller, 1, 3);
    assert!(!controller.is_holding());
    run_frames(&mut controller, 3, 90);
    assert!(controller.is_holding());
    assert!((controller.target_log2().unwrap() - 0.01_f64.log2() - 2.0).abs() <= 0.1);
}

#[test]
fn lock_freezes_by_timeout_when_the_measurement_never_settles() {
    let mut controller = controller();
    controller.begin_stream();
    controller.ingest(&measured(
        0,
        MeteredArea::WholeFrame,
        0.01,
        MeasurementQuality::ClippedHighlights,
    ));
    controller.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        ..directive()
    });
    controller.advance(ts(0));
    run_frames(&mut controller, 1, 55);
    assert!(
        !controller.is_holding(),
        "до таймаута (1 с = 60 кадров) цель не замораживается"
    );
    run_frames(&mut controller, 55, 65);
    assert!(controller.is_holding());
}

#[test]
fn locked_cold_start_catches_the_first_reliable_estimate_and_holds() {
    let mut controller = controller();
    controller.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        ..directive()
    });
    controller.begin_stream();
    controller.ingest(&whole_frame_measurement(0, 0.03));
    controller.advance(ts(0));
    controller.advance(ts(1));
    assert!(controller.is_holding());
    assert!((controller.target_log2().unwrap() - 0.03_f64.log2()).abs() < 1e-12);
}

#[test]
fn manual_authority_ignores_measurements_and_publishes_nothing() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(ExposureDirective {
        authority: ExposureAuthority::Manual,
        ..directive()
    });
    assert_eq!(
        controller.ingest(&whole_frame_measurement(1, 0.5)),
        IngestOutcome::IgnoredManual
    );
    controller.advance(ts(2));
    assert!(controller.proposal(ts(2), test_camera()).is_none());
    assert_eq!(controller.counters().ignored_manual, 1);
    assert!((controller.target_log2().unwrap() - 0.01_f64.log2()).abs() < 1e-12);
}

#[test]
fn failed_and_stale_measurements_change_nothing_and_are_counted() {
    let mut controller = started_at(directive(), 0.01);
    let before = controller.target_log2();
    let mut failed = whole_frame_measurement(1, 0.5);
    failed.outcome = Err(ExposureMeasurementError::ProductOutOfRange);
    assert_eq!(controller.ingest(&failed), IngestOutcome::MeasurementFailed);
    controller.ingest(&whole_frame_measurement(10, 0.01));
    assert_eq!(
        controller.ingest(&whole_frame_measurement(5, 0.5)),
        IngestOutcome::RejectedStale
    );
    assert_eq!(controller.target_log2(), before);
    let counters = controller.counters();
    assert_eq!((counters.failed, counters.rejected_stale), (1, 1));
    controller.note_skipped_measurement(SkippedMeasurement::NoPairedExposure);
    controller.note_skipped_measurement(SkippedMeasurement::MeterBusy);
    controller.note_skipped_measurement(SkippedMeasurement::MeterBusy);
    let counters = controller.counters();
    assert_eq!(
        (counters.skipped_no_paired_exposure, counters.skipped_meter_busy),
        (1, 2)
    );
}

#[test]
fn time_step_is_zero_out_of_order_and_clamped_after_a_long_gap() {
    let mut controller = started_at(directive(), 0.01);
    let start = 0.01_f64.log2();
    controller.ingest(&whole_frame_measurement(1, above(0.01, 2.0)));
    controller.advance(ts(100));
    controller.advance(ts(50)); // метка назад: шаг 0
    let after_out_of_order = controller.target_log2().unwrap();
    // Пауза в 100 кадров обрезана до 0,25 с: 1 − e^{-0,25/0,15} = 0,811 пути.
    let expected = start + 2.0 * (1.0 - (-0.25_f64 / 0.15).exp());
    assert!(
        (after_out_of_order - expected).abs() < 1e-6,
        "{after_out_of_order} против {expected}"
    );
    controller.advance(ts(40));
    assert_eq!(controller.target_log2().unwrap(), after_out_of_order);
}

#[test]
fn proposals_are_rate_limited_but_first_and_area_change_go_out_at_once() {
    let mut controller = started_at(directive(), 0.01);
    assert!(controller.proposal(ts(0), test_camera()).is_some(), "первое — сразу");
    assert!(
        controller.proposal(ts(0), test_camera()).is_none(),
        "без изменений — нет"
    );
    // Цель сдвинулась на 1 EV, но 20 мс < 50 мс: рано.
    controller.ingest(&whole_frame_measurement(1, above(0.01, 1.0)));
    run_frames(&mut controller, 1, 3);
    assert!(controller.proposal(ts(1), test_camera()).is_none());
    // Через >= 50 мс с прошлой публикации и сдвиге >= 1/24 EV — публикуем.
    run_frames(&mut controller, 3, 6);
    assert!(controller.proposal(ts(6), test_camera()).is_some());
    // Крошечный сдвиг (< 1/24 EV) не публикуется даже спустя время.
    controller.ingest(&whole_frame_measurement(7, above(0.01, 1.0 + 0.02)));
    run_frames(&mut controller, 7, 400);
    let settled = controller.proposal(ts(400), test_camera());
    assert!(
        settled.is_some(),
        "смена convergence/накопленный сдвиг дошли до публикации"
    );
    controller.ingest(&whole_frame_measurement(401, above(0.01, 1.0 + 0.02 + 0.01)));
    run_frames(&mut controller, 401, 800);
    assert!(
        controller.proposal(ts(800), test_camera()).is_none(),
        "сдвиг 0,01 EV < порога"
    );
    // Смена области — сразу, без ожидания интервала.
    controller.set_directive(tap(9));
    controller.ingest(&measured(
        801,
        MeteredArea::Point(PointRequestId(9)),
        0.02,
        MeasurementQuality::Measured,
    ));
    assert_eq!(
        controller.proposal(ts(801), test_camera()).unwrap().metered_area,
        MeteredArea::Point(PointRequestId(9))
    );
}

#[test]
fn begin_stream_forgets_target_and_republishes_first_proposal() {
    let mut controller = started_at(directive(), 0.01);
    assert!(controller.proposal(ts(0), test_camera()).is_some());
    controller.begin_stream();
    assert!(controller.target_log2().is_none());
    assert!(controller.proposal(ts(1), test_camera()).is_none());
    controller.ingest(&whole_frame_measurement(1, 0.04));
    assert!((controller.target_log2().unwrap() - 0.04_f64.log2()).abs() < 1e-12);
    assert!(
        controller.proposal(ts(1), test_camera()).is_some(),
        "первое предложение нового потока уходит сразу"
    );
}

pub(super) fn applied(seconds_iso: f64, shortfall_ev: f32) -> Option<AppliedExposure> {
    Some(AppliedExposure {
        product: product(seconds_iso),
        shortfall_ev,
        reach: ProductReach::Unbounded,
    })
}

#[test]
fn display_gain_bridges_the_gap_between_wanted_and_actual_exposure() {
    let mut controller = started_at(directive(), 0.02);
    controller.set_directive(ExposureDirective {
        applied: applied(0.02, 0.0),
        ..directive()
    });
    // Кадр темнее цели на 0,3 EV: подтяжка +0,3 EV.
    let frame = product(0.02 * (-0.3_f64).exp2());
    let gain = controller.display_gain(Some(frame));
    assert!((f64::from(gain.ev()) - 0.3).abs() < 1e-4, "{}", gain.ev());
    // Кадр ровно на цели: подтяжки нет.
    assert!(controller.display_gain(Some(product(0.02))).ev().abs() < 1e-5);
    // Слишком большой разрыв ограничен ±0,5 EV в обе стороны.
    assert!((controller.display_gain(Some(product(0.02 * (-3.0_f64).exp2()))).ev() - 0.5).abs() < 1e-5);
    assert!((controller.display_gain(Some(product(0.02 * 3.0_f64.exp2()))).ev() + 0.5).abs() < 1e-5);
}

#[test]
fn display_gain_includes_bias_and_ignores_the_lagging_shortfall() {
    let mut controller = started_at(directive(), 0.02);
    // Просили +0,5 EV компенсации; нехватка прошлого поколения (-3 EV) подтяжку не двигает.
    controller.set_directive(ExposureDirective {
        bias: ExposureBias::from_ev(0.5).unwrap(),
        applied: applied(0.02, -3.0),
        ..directive()
    });
    // D = T·2^0,5; кадр на уровне T → подтяжка +0,5 EV (зажим), а не 0 и не -0,5 от нехватки.
    let gain = controller.display_gain(Some(product(0.02)));
    assert!((gain.ev() - 0.5).abs() < 1e-4, "{}", gain.ev());
}

#[test]
fn display_gain_is_neutral_without_data_and_follows_applied_product_in_manual() {
    let mut controller = controller();
    controller.begin_stream();
    // Нет цели, нет указания с applied, нет кадра — везде UNITY.
    assert_eq!(controller.display_gain(Some(product(0.02))), ExposureGain::UNITY);
    controller.ingest(&whole_frame_measurement(0, 0.02));
    assert_eq!(
        controller.display_gain(Some(product(0.02))),
        ExposureGain::UNITY,
        "applied нет"
    );
    controller.set_directive(ExposureDirective {
        applied: applied(0.02, 0.0),
        ..directive()
    });
    assert_eq!(
        controller.display_gain(None),
        ExposureGain::UNITY,
        "нет произведения кадра"
    );
    // Ручная экспозиция: D = applied.product, цель замера не нужна.
    controller.begin_stream();
    controller.set_directive(ExposureDirective {
        authority: ExposureAuthority::Manual,
        applied: applied(0.01, 0.0),
        ..directive()
    });
    let gain = controller.display_gain(Some(product(0.01 * 0.2_f64.exp2())));
    assert!((gain.ev() + 0.2).abs() < 1e-4, "{}", gain.ev());
}

#[test]
fn display_gain_is_constant_while_holding() {
    let mut controller = started_at(directive(), 0.02);
    controller.set_directive(ExposureDirective {
        lock: AeAfLock::Locked,
        applied: applied(0.02, 0.0),
        ..directive()
    });
    controller.ingest(&whole_frame_measurement(1, 0.02));
    controller.advance(ts(1));
    let frame = Some(product(0.02 * (-0.2_f64).exp2()));
    let first = controller.display_gain(frame);
    controller.ingest(&whole_frame_measurement(2, above(0.02, 3.0)));
    run_frames(&mut controller, 2, 100);
    assert_eq!(controller.display_gain(frame), first);
}

#[path = "controller_observation_tests.rs"]
mod observation;
