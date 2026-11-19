//! Разбор ступеньки `ae-trace` на синтетических рядах: каждое число проверяется на ряде, где известен ответ.

use super::super::ae_trace_test_support::flat_sample;
use super::*;

const CUE_MS: u64 = 6_000;
const END_MS: u64 = 12_000;

/// Реакция владельца: замер сцены уходит со ступеньки через столько мс после подсказки.
const REACTION_MS: u64 = 500;

/// Ряд с шагом 50 мс от 0 до `END_MS`: замер, уровень сенсора и подтяжка задаются функциями времени.
fn run(observed: impl Fn(u64) -> f64, sensor: impl Fn(u64) -> f64, gain: impl Fn(u64) -> f64) -> Vec<TraceSample> {
    (0..END_MS / 50)
        .map(|index| {
            let t_ms = index * 50;
            TraceSample {
                observed_ev: Some(observed(t_ms)),
                sensor_ev: Some(sensor(t_ms)),
                // Здесь показанный кадр совпадает с последним результатом HAL; расхождение — в отдельном тесте.
                shown_ev: Some(sensor(t_ms)),
                gain_ev: gain(t_ms),
                ..flat_sample(t_ms, 0.0)
            }
        })
        .collect()
}

/// Ступенька замера на `size_ev` через `REACTION_MS` после подсказки.
fn scene_step(size_ev: f64) -> impl Fn(u64) -> f64 {
    move |t_ms| if t_ms < CUE_MS + REACTION_MS { 0.0 } else { size_ev }
}

/// Монотонное приближение сенсора к ступеньке с постоянной времени `tau_ms`.
fn exponential(size_ev: f64, tau_ms: f64) -> impl Fn(u64) -> f64 {
    move |t_ms| {
        let since = t_ms.saturating_sub(CUE_MS + REACTION_MS) as f64;
        size_ev * (1.0 - (-since / tau_ms).exp())
    }
}

fn analyze(samples: &[TraceSample]) -> StepResult {
    analyze_step(
        &AeTraceScenarioConfig::default(),
        samples,
        Segment {
            cue_index: 0,
            cue_ms: CUE_MS,
            end_ms: END_MS,
        },
    )
}

fn metrics(result: &StepResult) -> StepMetrics {
    result.metrics.expect("у настоящей ступеньки есть метрики")
}

#[test]
fn clean_step_is_measured_with_size_onset_and_settle_time() {
    let samples = run(scene_step(1.0), exponential(1.0, 300.0), |_| 0.0);

    let result = analyze(&samples);

    assert_eq!(result.kind, StepKind::Measured);
    assert!((result.step_ev.expect("ступенька") - 1.0).abs() < 1e-9);
    let m = metrics(&result);
    assert_eq!(
        m.onset_ms,
        Some(REACTION_MS),
        "замер ушёл с базового уровня через реакцию владельца"
    );
    // Вход в полосу 0,1 EV: 1 - e^(-t/0,3) > 0,9 -> t > 0,3 * ln(10) = 0,69 с после начала ступеньки.
    let settle = m.sensor_settle_ms.expect("установился");
    assert!(
        (REACTION_MS + 650..=REACTION_MS + 800).contains(&settle),
        "установка {settle} мс от подсказки"
    );
    assert_eq!(
        m.screen_settle_ms, m.sensor_settle_ms,
        "без подтяжки экран повторяет сенсор"
    );
    assert!(
        m.overshoot_ev < 1e-6,
        "монотонное приближение не перерегулирует: {}",
        m.overshoot_ev
    );
    assert_eq!(m.max_abs_gain_ev, 0.0);
}

#[test]
fn slower_profile_settles_later_than_a_fast_one() {
    let fast = analyze(&run(scene_step(1.0), exponential(1.0, 150.0), |_| 0.0));
    let slow = analyze(&run(scene_step(1.0), exponential(1.0, 800.0), |_| 0.0));
    let (fast_ms, slow_ms) = (
        metrics(&fast).sensor_settle_ms.expect("быстрый установился"),
        metrics(&slow).sensor_settle_ms.expect("медленный установился"),
    );
    assert!(
        slow_ms > fast_ms + 1_000,
        "видео-τ 0,8 с против фото-τ 0,15 с: {slow_ms} против {fast_ms}"
    );
}

#[test]
fn overshoot_is_the_excursion_past_the_final_value_in_the_step_direction() {
    // Сенсор проскакивает до 1,3 EV, потом возвращается к 1,0.
    let sensor = |t_ms: u64| {
        if t_ms < CUE_MS + REACTION_MS {
            return 0.0;
        }
        let since = (t_ms - CUE_MS - REACTION_MS) as f64;
        1.0 + 0.3 * (-since / 400.0).exp()
    };
    let up = analyze(&run(scene_step(1.0), sensor, |_| 0.0));
    let overshoot = metrics(&up).sensor_overshoot_ev;
    assert!((overshoot - 0.3).abs() < 0.02, "перерегулирование {overshoot}");
    assert!(
        (metrics(&up).overshoot_ev - overshoot).abs() < 1e-9,
        "без подтяжки экран и сенсор совпадают"
    );

    // Сцена светлее: ступенька -1 EV, проскок вниз до -1,3 тоже считается заходом «в сторону ступеньки».
    let mirrored = |t_ms: u64| -sensor(t_ms);
    let down = analyze(&run(scene_step(-1.0), mirrored, |_| 0.0));
    assert!(
        (metrics(&down).sensor_overshoot_ev - 0.3).abs() < 0.02,
        "{:?}",
        metrics(&down)
    );
    assert!(down.step_ev.expect("ступенька") < 0.0);
}

#[test]
fn residual_gain_can_remove_overshoot_on_screen_while_the_sensor_still_has_it() {
    let sensor = |t_ms: u64| {
        if t_ms < CUE_MS + REACTION_MS {
            0.0
        } else if t_ms < CUE_MS + 1_500 {
            1.3
        } else {
            1.0
        }
    };
    // Подтяжка гасит проскок, пока сенсор на 1,3, и возвращается к нулю.
    let gain = |t_ms: u64| {
        if (CUE_MS + REACTION_MS..CUE_MS + 1_500).contains(&t_ms) {
            -0.3
        } else {
            0.0
        }
    };

    let result = analyze(&run(scene_step(1.0), sensor, gain));

    let m = metrics(&result);
    assert!((m.sensor_overshoot_ev - 0.3).abs() < 1e-9);
    assert!(m.overshoot_ev < 1e-9, "на экране проскока нет: {}", m.overshoot_ev);
    assert!((m.max_abs_gain_ev - 0.3).abs() < 1e-9);
    assert_eq!(m.gain_at_end_ev, Some(0.0), "после схождения подтяжка вернулась к нулю");
}

#[test]
fn gain_left_non_zero_at_the_end_is_reported() {
    let result = analyze(&run(scene_step(1.0), exponential(1.0, 300.0), |t_ms| {
        if t_ms > CUE_MS { 0.25 } else { 0.0 }
    }));
    assert_eq!(metrics(&result).gain_at_end_ev, Some(0.25));
}

#[test]
fn settle_time_is_measured_from_the_last_exit_from_the_band_not_the_first_entry() {
    // Вход в полосу на 1000, выход на 1500, окончательный вход на 2000 (время от подсказки).
    let series: Vec<(u64, f64)> = (0..60)
        .map(|index| {
            let since = index * 50;
            let inside = (1_000..1_500).contains(&since) || since >= 2_000;
            (CUE_MS + since, if inside { 1.0 } else { 0.5 })
        })
        .collect();
    assert_eq!(settle_ms(&series, CUE_MS, 1.0, 0.1), Some(2_000));
}

#[test]
fn series_that_ends_outside_the_band_never_settled() {
    let series = vec![(CUE_MS, 0.0), (CUE_MS + 50, 1.0), (CUE_MS + 100, 0.5)];
    assert_eq!(settle_ms(&series, CUE_MS, 1.0, 0.1), None);
    assert_eq!(settle_ms(&[], CUE_MS, 1.0, 0.1), None);
}

#[test]
fn oscillation_until_the_end_gives_no_settle_time() {
    let samples = run(
        scene_step(1.0),
        |t_ms| if (t_ms / 50) % 2 == 0 { 1.5 } else { 0.5 },
        |_| 0.0,
    );
    let result = analyze(&samples);
    assert_eq!(result.kind, StepKind::Measured);
    assert_eq!(metrics(&result).sensor_settle_ms, None);
}

#[test]
fn missed_cue_is_no_step_and_carries_no_metrics() {
    let samples = run(|_| 0.1, |_| 0.0, |_| 0.0);
    let result = analyze(&samples);
    assert_eq!(result.kind, StepKind::NoStep);
    assert!(result.metrics.is_none());
    assert!(result.step_ev.expect("размер известен").abs() < 0.5);
}

#[test]
fn step_just_below_the_threshold_is_missed_and_just_above_is_measured() {
    assert_eq!(
        analyze(&run(scene_step(0.49), exponential(0.49, 300.0), |_| 0.0)).kind,
        StepKind::NoStep
    );
    assert_eq!(
        analyze(&run(scene_step(0.51), exponential(0.51, 300.0), |_| 0.0)).kind,
        StepKind::Measured
    );
}

#[test]
fn thin_windows_are_insufficient_data_not_an_invented_number() {
    // Замера сцены в последнем окне нет вовсе.
    let mut samples = run(scene_step(1.0), exponential(1.0, 300.0), |_| 0.0);
    for sample in samples.iter_mut().filter(|sample| sample.t_ms >= END_MS - 1_000) {
        sample.observed_ev = None;
    }
    let result = analyze(&samples);
    assert_eq!(result.kind, StepKind::InsufficientData);
    assert_eq!(result.step_ev, None);
    assert!(result.metrics.is_none());
    // Ряд короче окна до подсказки.
    assert_eq!(analyze(&[]).kind, StepKind::InsufficientData);
}

#[test]
fn counters_are_taken_inside_the_segment_only() {
    let mut samples = run(scene_step(1.0), exponential(1.0, 300.0), |_| 0.0);
    for sample in &mut samples {
        // 2 предложения в секунду всё время; поколение растёт вдвое медленнее.
        sample.proposals_published = sample.t_ms / 500;
        sample.directive_generation = Some(sample.t_ms / 1_000);
    }
    let m = metrics(&analyze(&samples));
    assert_eq!(
        m.proposals_published,
        (END_MS - 50) / 500 - CUE_MS / 500,
        "рост внутри отрезка, не за весь ряд"
    );
    assert_eq!(m.directive_generations, Some((END_MS - 50) / 1_000 - CUE_MS / 1_000));
}

#[test]
fn hal_result_ahead_of_the_shown_frame_gives_no_false_screen_overshoot() {
    // Ступенька +1 EV. Показанный кадр доезжает до 1,0 позже последнего результата HAL (на 300 мс), а подтяжка
    // кадра гасит остаток разрыва. Сумма «HAL + подтяжка» дала бы проскок 0,3 EV; настоящий экран монотонен.
    let hal = |t_ms: u64| exponential(1.0, 150.0)(t_ms);
    let shown = |t_ms: u64| exponential(1.0, 150.0)(t_ms.saturating_sub(300));
    let gain = |t_ms: u64| (hal(t_ms) - shown(t_ms)).clamp(0.0, 0.3);
    let samples: Vec<TraceSample> = (0..END_MS / 50)
        .map(|index| {
            let t_ms = index * 50;
            TraceSample {
                observed_ev: Some(scene_step(1.0)(t_ms)),
                sensor_ev: Some(hal(t_ms)),
                shown_ev: Some(shown(t_ms)),
                gain_ev: gain(t_ms),
                ..flat_sample(t_ms, 0.0)
            }
        })
        .collect();

    let m = metrics(&analyze(&samples));

    assert!(
        m.overshoot_ev < 0.05,
        "ложное перерегулирование экрана: {}",
        m.overshoot_ev
    );
}
