//! Отсчёты `ae-trace`: перевод показаний в EV, учёт новых кадров, коды, предел ёмкости.

use camera_contract::auto_exposure::ProposalConvergence;

use super::super::ae_trace_test_support::{diagnostics, flat_sample, hal_result};
use super::*;

#[test]
fn product_is_log2_of_seconds_times_iso() {
    // 10 мс × ISO 100 = 1 с·ISO = 0 EV; вдвое больше — +1 EV.
    let base = product_ev(Duration::from_millis(10), 100).expect("есть произведение");
    assert!(base.abs() < 1e-9, "{base}");
    let doubled = product_ev(Duration::from_millis(20), 100).expect("есть произведение");
    assert!((doubled - 1.0).abs() < 1e-9, "{doubled}");
    assert_eq!(product_ev(Duration::ZERO, 100), None);
    assert_eq!(product_ev(Duration::from_millis(10), 0), None);
}

#[test]
fn sample_carries_the_diagnostics_and_the_sensor_level_of_the_repeating_frame() {
    let mut taker = SampleTaker::default();
    let diag = diagnostics(Some(1.5), Some(1.25), 0.125, 7, Some(3));
    let result = hal_result(10, RequestOrigin::Repeating, 20.0, 100, Some(SceneFlicker::Mains50Hz));

    let sample = taker.take(250, &diag, Some(&result), ThreeAProfile::Video);

    assert_eq!(sample.t_ms, 250);
    assert_eq!(sample.observed_ev, Some(1.5));
    assert_eq!(sample.target_ev, Some(1.25));
    assert!((sample.sensor_ev.expect("уровень сенсора") - 1.0).abs() < 1e-9);
    assert_eq!(sample.gain_ev, 0.125);
    assert!((sample.screen_ev().expect("экран") - 1.125).abs() < 1e-9);
    assert_eq!(sample.convergence, Some(ProposalConvergence::Settled));
    assert_eq!((sample.proposals_published, sample.directive_generation), (7, Some(3)));
    assert_eq!(sample.profile, ThreeAProfile::Video);
    let frame = sample.new_frame.expect("кадр новый");
    assert_eq!(frame.flicker, FlickerReading::Mains50Hz);
    assert!((frame.exposure_ms.expect("выдержка") - 20.0).abs() < 1e-6);
}

#[test]
fn the_same_frame_is_counted_once_and_the_sensor_level_is_held_between_frames() {
    let mut taker = SampleTaker::default();
    let diag = diagnostics(Some(0.0), Some(0.0), 0.0, 0, None);
    let result = hal_result(10, RequestOrigin::Repeating, 10.0, 100, None);

    let first = taker.take(0, &diag, Some(&result), ThreeAProfile::Photo);
    let again = taker.take(50, &diag, Some(&result), ThreeAProfile::Photo);

    assert!(first.new_frame.is_some());
    assert_eq!(
        again.new_frame, None,
        "тот же номер кадра на следующем тике — не новый кадр"
    );
    assert_eq!(
        again.sensor_ev, first.sensor_ev,
        "уровень сенсора держится до следующего кадра"
    );
    assert_eq!(
        again.latest_flicker, first.latest_flicker,
        "ответ детектора держится между кадрами"
    );
}

#[test]
fn non_repeating_results_do_not_enter_the_series() {
    let mut taker = SampleTaker::default();
    let diag = diagnostics(Some(0.0), Some(0.0), 0.0, 0, None);
    let burst = hal_result(
        11,
        RequestOrigin::Burst {
            burst: camera_contract::three_a::BurstId(1),
            index: 0,
        },
        100.0,
        800,
        None,
    );

    let sample = taker.take(0, &diag, Some(&burst), ThreeAProfile::Photo);

    assert_eq!(sample.new_frame, None);
    assert_eq!(
        sample.sensor_ev, None,
        "экспозиция кадра серии не принимается за уровень сенсора"
    );
}

#[test]
fn absent_flicker_key_is_distinct_from_the_none_answer() {
    let mut taker = SampleTaker::default();
    let diag = diagnostics(None, None, 0.0, 0, None);
    let absent = hal_result(1, RequestOrigin::Repeating, 5.0, 100, None);
    let none = hal_result(2, RequestOrigin::Repeating, 5.0, 100, Some(SceneFlicker::None));

    let first = taker.take(0, &diag, Some(&absent), ThreeAProfile::Photo);
    let second = taker.take(50, &diag, Some(&none), ThreeAProfile::Photo);

    assert_eq!(first.new_frame.map(|frame| frame.flicker), Some(FlickerReading::Absent));
    assert_eq!(second.new_frame.map(|frame| frame.flicker), Some(FlickerReading::None));
}

#[test]
fn codes_are_unique_and_follow_the_legend() {
    let codes: Vec<u8> = FlickerReading::ALL.iter().map(|reading| reading.code()).collect();
    assert_eq!(codes, vec![0, 1, 2, 3]);
    assert_eq!(convergence_code(None), 0);
    assert_eq!(convergence_code(Some(ProposalConvergence::Searching)), 1);
    assert_eq!(convergence_code(Some(ProposalConvergence::Settled)), 2);
    assert_eq!(convergence_code(Some(ProposalConvergence::Holding)), 3);
}

#[test]
fn full_series_drops_and_counts_instead_of_growing() {
    let mut series = SeriesBuffer::with_capacity(2);
    assert_eq!(series.push(flat_sample(0, 0.0)), PushOutcome::Stored);
    assert_eq!(series.push(flat_sample(50, 0.0)), PushOutcome::Stored);
    assert_eq!(series.push(flat_sample(100, 0.0)), PushOutcome::DroppedFull);
    assert_eq!(series.samples().len(), 2);
    assert_eq!(series.dropped(), 1);
}
