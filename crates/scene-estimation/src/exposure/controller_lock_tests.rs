//! Контроллер цели AE: блокировка AE/AF — поиск и удержание при схождении, возврат из ручного
//! режима, ожидание свежего замера, таймаут заморозки, гистерезис «установившейся» оценки и фаза замера.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::auto_exposure::MeteredArea;

use super::tests::{
    above, controller, directive, measured, run_frames, started_at, ts, video, whole_frame_measurement,
};
use super::*;
use crate::exposure::measurement::MeasurementPhase;
use crate::exposure::test_support::test_camera;

fn lock_directive(base: ExposureDirective) -> ExposureDirective {
    ExposureDirective {
        lock: AeAfLock::Locked,
        ..base
    }
}

#[test]
fn video_lock_is_searching_while_settling_and_holding_afterwards() {
    let mut controller = started_at(video(), 0.01);
    // Сцена ярче на 1 EV, блокировка запрошена до схождения.
    controller.set_directive(lock_directive(video()));
    controller.ingest(&whole_frame_measurement(1, above(0.01, 1.0)));
    controller.advance(ts(1));
    let early = controller.proposal(ts(1), test_camera()).unwrap();
    assert_eq!(
        early.convergence,
        ProposalConvergence::Searching,
        "досчёт блокировки не Settled"
    );
    let mut seen_searching_until_holding = true;
    for frame in 2..120 {
        controller.advance(ts(frame));
        if controller
            .proposal(ts(frame), test_camera())
            .is_some_and(|proposal| proposal.convergence == ProposalConvergence::Settled)
        {
            seen_searching_until_holding = false;
        }
    }
    assert!(seen_searching_until_holding, "Settled появился до Holding");
    assert_eq!(
        controller
            .proposal(ts(300), test_camera())
            .map(|p| p.convergence)
            .unwrap_or(ProposalConvergence::Holding),
        ProposalConvergence::Holding
    );
    assert!(controller.is_holding());
}

#[test]
fn returning_from_manual_does_not_chase_a_stale_scene_estimate() {
    let mut controller = started_at(directive(), 0.01);
    let frozen = controller.target_log2().unwrap();
    controller.set_directive(ExposureDirective {
        authority: ExposureAuthority::Manual,
        ..directive()
    });
    // Пока ручной режим, сцена изменилась, но замеры игнорируются.
    controller.ingest(&whole_frame_measurement(1, above(0.01, 3.0)));
    controller.set_directive(directive());
    run_frames(&mut controller, 2, 200);
    assert_eq!(
        controller.target_log2().unwrap(),
        frozen,
        "цель стоит до свежего замера"
    );
    assert!(
        controller.proposal(ts(200), test_camera()).is_none(),
        "публикации по устаревшей оценке нет"
    );
    controller.ingest(&whole_frame_measurement(201, above(0.01, 3.0)));
    run_frames(&mut controller, 201, 400);
    assert!((controller.target_log2().unwrap() - frozen - 3.0).abs() < 0.05);
}

#[test]
fn lock_in_the_same_area_waits_for_a_fresh_measurement() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(lock_directive(directive()));
    // Старая оценка совпала с целью, но свежего замера после запроса блокировки не было.
    run_frames(&mut controller, 1, 30);
    assert!(!controller.is_holding(), "заморозка по давнишней оценке");
    controller.ingest(&whole_frame_measurement(30, above(0.01, 1.0)));
    controller.advance(ts(30));
    assert!(!controller.is_holding(), "цель ещё не дошла до свежей оценки");
    run_frames(&mut controller, 31, 120);
    assert!(controller.is_holding());
    assert!((controller.target_log2().unwrap() - 0.01_f64.log2() - 1.0).abs() <= 0.1);
}

#[test]
fn lock_still_freezes_by_timeout_without_any_fresh_measurement() {
    let mut controller = started_at(directive(), 0.01);
    controller.set_directive(lock_directive(directive()));
    run_frames(&mut controller, 1, 55);
    assert!(!controller.is_holding());
    run_frames(&mut controller, 55, 70);
    assert!(controller.is_holding());
}

#[test]
fn settled_has_hysteresis_so_jitter_near_the_tolerance_does_not_republish() {
    // Цель практически неподвижна (огромная τ): дрожит только оценка сцены относительно неё.
    let mut controller = AutoExposureController::new(ExposureControllerConfig {
        photo_time_constant_seconds: 1.0e6,
        ..ExposureControllerConfig::default()
    })
    .unwrap();
    controller.begin_stream();
    controller.ingest(&whole_frame_measurement(0, 0.01));
    controller.advance(ts(0));
    assert_eq!(
        controller.proposal(ts(0), test_camera()).unwrap().convergence,
        ProposalConvergence::Settled
    );
    // Оценка дрожит между 0,05 и 0,15 EV от цели: между допуском (0,1) и порогом выхода (0,2).
    let mut published = 0;
    let mut frame = 1;
    for round in 0..40 {
        let jitter = if round % 2 == 0 { 0.15 } else { 0.05 };
        controller.ingest(&whole_frame_measurement(frame, above(0.01, jitter)));
        // Цель мгновенно не идёт к оценке: зафиксируем её у границы, пересчитывая сходимость на месте.
        controller.advance(ts(frame));
        if let Some(proposal) = controller.proposal(ts(frame + 6), test_camera()) {
            published += 1;
            assert_ne!(proposal.convergence, ProposalConvergence::Holding);
        }
        frame += 6;
    }
    assert!(published <= 3, "публикаций {published}: Settled/Searching дрожит");
    // Явный выход за порог 0,2 EV снимает Settled.
    let mut far = started_at(directive(), 0.01);
    far.ingest(&whole_frame_measurement(1, above(0.01, 0.5)));
    assert_eq!(
        far.proposal(ts(1), test_camera()).unwrap().convergence,
        ProposalConvergence::Searching
    );
}

#[test]
fn the_measurement_phase_stays_cold_until_the_first_reliable_estimate_of_the_stream() {
    let mut controller = controller();
    assert_eq!(controller.measurement_phase(), MeasurementPhase::ColdStart);
    // Пересвеченная оценка не надёжна: фаза прежняя.
    let clipped = measured(0, MeteredArea::WholeFrame, 1.0, MeasurementQuality::ClippedHighlights);
    assert_eq!(controller.ingest(&clipped), IngestOutcome::Accepted);
    assert_eq!(controller.measurement_phase(), MeasurementPhase::ColdStart);
    // Надёжная — фаза установившаяся.
    let reliable = measured(1, MeteredArea::WholeFrame, 1.0, MeasurementQuality::Measured);
    assert_eq!(controller.ingest(&reliable), IngestOutcome::Accepted);
    assert_eq!(controller.measurement_phase(), MeasurementPhase::Established);
    // Новый поток начинает холодный старт заново.
    controller.begin_stream();
    assert_eq!(controller.measurement_phase(), MeasurementPhase::ColdStart);
}
