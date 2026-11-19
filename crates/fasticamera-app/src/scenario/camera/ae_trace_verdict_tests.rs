//! Вердикт `ae-trace`: что считается нарушением, а что только справкой, и граница каждого порога.

use camera_contract::three_a::ThreeAProfile;
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticRealizationKind};

use super::super::ae_trace_analysis::analyze_run;
use super::super::ae_trace_plan::Timeline;
use super::super::ae_trace_series::TraceSample;
use super::super::ae_trace_test_support::flat_sample;
use super::*;

/// Предел подтяжки контроллера по умолчанию, EV.
const GAIN_LIMIT_EV: f64 = 0.5;

fn config() -> AeTraceScenarioConfig {
    AeTraceScenarioConfig {
        photo_phase_ms: 4_000,
        video_phase_ms: 4_000,
        static_phase_ms: 4_000,
        cue_interval_ms: 2_000,
        static_warmup_ms: 1_000,
        step_window_ms: 500,
        series_capacity: 1_000,
        ..AeTraceScenarioConfig::default()
    }
}

/// Здоровый прогон: на подсказке сцена и сенсор мгновенно переходят 0 -> 1 -> 0, подтяжки нет, профили верные.
fn healthy_series(config: &AeTraceScenarioConfig) -> Vec<TraceSample> {
    let timeline = Timeline::from_config(config);
    (0..timeline.total_ms() / 50)
        .map(|index| {
            let t_ms = index * 50;
            let window = timeline
                .phases()
                .iter()
                .find(|window| (window.start_ms..window.end_ms).contains(&t_ms))
                .expect("момент внутри расписания");
            let level = (window.cues_ms.iter().filter(|cue| **cue <= t_ms).count() % 2) as f64;
            TraceSample {
                profile: window.phase.profile(),
                ..flat_sample(t_ms, level)
            }
        })
        .collect()
}

fn observe(config: &AeTraceScenarioConfig, series: Vec<TraceSample>) -> AeTraceObservation {
    let timeline = Timeline::from_config(config);
    let analysis = analyze_run(config, &timeline, &series);
    AeTraceObservation {
        gain_limit_ev: GAIN_LIMIT_EV,
        timeline: Some(timeline),
        series,
        analysis: Some(analysis),
        ..AeTraceObservation::default()
    }
}

fn violations_of(series: Vec<TraceSample>) -> Vec<String> {
    let config = config();
    evaluate_ae_trace(&config, &observe(&config, series))
}

#[test]
fn healthy_run_has_no_violations() {
    let config = config();
    assert_eq!(violations_of(healthy_series(&config)), Vec::<String>::new());
}

#[test]
fn residual_gain_over_the_controller_limit_plus_epsilon_fails_and_at_the_limit_passes() {
    let config = config();
    let with_gain = |gain_ev: f64| {
        let mut series = healthy_series(&config);
        series[30].gain_ev = gain_ev;
        violations_of(series)
    };
    // Предел 0,5 + допуск 0,02: ровно 0,52 ещё проходит, 0,53 — нет; знак значения не важен.
    assert!(with_gain(0.52).is_empty());
    assert!(with_gain(-0.52).is_empty());
    for failing in [0.53, -0.53] {
        let violations = with_gain(failing);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("подтяжка ISP"), "{violations:?}");
    }
}

#[test]
fn the_gain_limit_comes_from_the_observation_not_from_a_constant() {
    let config = config();
    let mut series = healthy_series(&config);
    series[30].gain_ev = 0.4;
    let mut observation = observe(&config, series);
    assert!(evaluate_ae_trace(&config, &observation).is_empty());
    observation.gain_limit_ev = 0.25;
    assert_eq!(
        evaluate_ae_trace(&config, &observation).len(),
        1,
        "контроллер с пределом 0,25 EV"
    );
}

#[test]
fn overshoot_over_the_threshold_on_a_real_step_fails_and_below_passes() {
    let config = config();
    let with_overshoot = |size_ev: f64| {
        let mut series = healthy_series(&config);
        // Подсказка на 2000: сенсор на 2000..2200 проскакивает конечный уровень 1,0 на `size_ev`.
        for sample in series.iter_mut().filter(|sample| (2_000..2_200).contains(&sample.t_ms)) {
            sample.sensor_ev = Some(1.0 + size_ev);
        }
        violations_of(series)
    };
    assert!(with_overshoot(0.10).is_empty(), "0,10 < порога 0,15");
    let violations = with_overshoot(0.20);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("фаза photo, подсказка 1"), "{violations:?}");
    assert!(violations[0].contains("перерегулирование"), "{violations:?}");
}

#[test]
fn a_missed_cue_is_not_judged_even_if_the_series_wiggles() {
    // Владелец ничего не переключал: замер ровный, а сенсор дёргается — это не ступенька.
    let mut series: Vec<TraceSample> = (0..240)
        .map(|index| TraceSample {
            profile: ThreeAProfile::Photo,
            ..flat_sample(index * 50, 0.0)
        })
        .collect();
    for sample in series.iter_mut().filter(|sample| (2_000..2_200).contains(&sample.t_ms)) {
        sample.sensor_ev = Some(0.4);
    }
    for sample in series.iter_mut().filter(|sample| (4_000..8_000).contains(&sample.t_ms)) {
        sample.profile = ThreeAProfile::Video;
    }
    // Ступенек не было вовсе — значит, и приёмки не было: единственное нарушение именно об этом.
    let violations = violations_of(series);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("ни одной настоящей ступеньки"), "{violations:?}");
}

#[test]
fn unconfirmed_profile_fails_the_phase() {
    let config = config();
    let mut series = healthy_series(&config);
    for sample in series.iter_mut().filter(|sample| (4_000..8_000).contains(&sample.t_ms)) {
        sample.profile = ThreeAProfile::Photo;
    }
    let violations = violations_of(series);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].starts_with("фаза video"), "{violations:?}");
}

#[test]
fn harness_problems_are_violations_even_without_an_analysis() {
    let config = config();
    let observation = AeTraceObservation {
        failure: Some("нет диагностики AE: вендорский AE у руля".to_owned()),
        restore_failure: Some("очередь переполнена".to_owned()),
        dropped_samples: 3,
        ..AeTraceObservation::default()
    };
    let violations = evaluate_ae_trace(&config, &observation);
    assert_eq!(violations.len(), 3, "{violations:?}");
    assert!(violations[0].contains("вендорский AE"));
    assert!(violations[1].contains("не возвращён"));
    assert!(violations[2].contains("потеряно 3"));
}

#[test]
fn undelivered_cues_are_reported_with_the_count_and_the_reason() {
    let config = config();
    let mut observation = observe(&config, healthy_series(&config));
    observation.haptics.record(
        "старт photo",
        HapticPattern::Success,
        HapticOutcome::Queued(HapticRealizationKind::Waveform),
    );
    observation
        .haptics
        .record("подсказка 1", HapticPattern::SelectionTick, HapticOutcome::Backpressure);
    observation
        .haptics
        .record("подсказка 2", HapticPattern::SelectionTick, HapticOutcome::NotAttached);

    let violations = evaluate_ae_trace(&config, &observation);

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("2 из 3"), "{violations:?}");
    assert!(
        violations[0].contains("подсказка 1") && violations[0].contains("Backpressure"),
        "{violations:?}"
    );
}
