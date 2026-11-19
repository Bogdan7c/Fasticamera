//! Малые расчёты `ae-trace`: окна, медиана, размах, кратность периоду мерцания, «дыхание».

use super::super::ae_trace_test_support::{flat_sample, new_frame, series};
use super::*;

#[test]
fn window_is_half_open_and_survives_empty_and_inverted_ranges() {
    let samples = series(100, 10, |_| 0.0);
    let times: Vec<u64> = window(&samples, 200, 500).iter().map(|sample| sample.t_ms).collect();
    assert_eq!(times, vec![200, 300, 400], "левая граница включена, правая нет");
    assert!(window(&samples, 5_000, 6_000).is_empty());
    assert!(
        window(&samples, 500, 200).is_empty(),
        "перевёрнутый диапазон пуст, а не паника"
    );
}

#[test]
fn median_and_spread_cover_odd_even_and_empty_sets() {
    assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    assert_eq!(median(&[]), None);
    assert_eq!(spread(&[1.0, 4.0, 2.5]), Some(3.0));
    assert_eq!(spread(&[]), None);
}

#[test]
fn counters_report_growth_inside_the_range_only() {
    let mut samples = series(100, 5, |_| 0.0);
    for (index, sample) in samples.iter_mut().enumerate() {
        sample.proposals_published = 10 + index as u64 * 3;
        sample.directive_generation = Some(4 + index as u64);
    }
    assert_eq!(counter_delta(&samples), 12);
    assert_eq!(generation_delta(&samples), Some(4));
    assert_eq!(counter_delta(&samples[2..3]), 0, "один отсчёт — роста нет");
    assert_eq!(counter_delta(&[]), 0);
    let none: Vec<TraceSample> = samples
        .iter()
        .map(|sample| TraceSample {
            directive_generation: None,
            ..*sample
        })
        .collect();
    assert_eq!(
        generation_delta(&none),
        None,
        "поколения не было — это не «0 перестроек»"
    );
}

#[test]
fn exposure_multiple_of_period_tolerates_line_quantization_only() {
    let config = AeTraceScenarioConfig::default();
    assert!(is_multiple_of_period(10.0, &config));
    assert!(is_multiple_of_period(9.97, &config), "до допуска ниже кратной");
    assert!(is_multiple_of_period(20.03, &config), "до допуска выше кратной");
    assert!(!is_multiple_of_period(15.0, &config));
    assert!(!is_multiple_of_period(10.2, &config), "дальше допуска — не кратна");
}

#[test]
fn flicker_stats_count_readings_long_frames_and_the_multiple_share() {
    let config = AeTraceScenarioConfig::default();
    let frames = [
        (Some(4.0), FlickerReading::Mains50Hz),  // короткий: в долю не входит
        (Some(10.0), FlickerReading::Mains50Hz), // длинный, кратный
        (Some(20.0), FlickerReading::Mains60Hz), // длинный, кратный
        (Some(13.0), FlickerReading::None),      // длинный, не кратный
        (None, FlickerReading::Absent),          // выдержки нет
    ];
    let samples: Vec<TraceSample> = frames
        .iter()
        .enumerate()
        .map(|(index, (exposure, reading))| TraceSample {
            new_frame: new_frame(*exposure, *reading),
            ..flat_sample(index as u64 * 50, 0.0)
        })
        .chain(std::iter::once(flat_sample(500, 0.0)))
        .collect();

    let stats = flicker_stats(&config, &samples);

    assert_eq!(stats.readings["mains_50hz"], 2);
    assert_eq!(stats.readings["mains_60hz"], 1);
    assert_eq!(stats.readings["none"], 1);
    assert_eq!(stats.readings["absent"], 1);
    assert_eq!(stats.long_exposure_frames, 3);
    assert_eq!(stats.multiple_of_period_frames, 2);
    assert_eq!(stats.multiple_share(), Some(2.0 / 3.0));
    assert_eq!(stats.max_exposure_ms, Some(20.0));
}

#[test]
fn flicker_share_is_unknown_without_long_frames() {
    let config = AeTraceScenarioConfig::default();
    let samples = vec![TraceSample {
        new_frame: new_frame(Some(3.0), FlickerReading::None),
        ..flat_sample(0, 0.0)
    }];
    let stats = flicker_stats(&config, &samples);
    assert_eq!(stats.multiple_share(), None);
    assert_eq!(stats.max_exposure_ms, Some(3.0));
}

#[test]
fn quiet_stats_measure_ranges_and_rates_per_second() {
    let mut samples = series(500, 5, |t_ms| if t_ms == 1_000 { 0.25 } else { 0.0 });
    for (index, sample) in samples.iter_mut().enumerate() {
        sample.gain_ev = if index == 3 { -0.125 } else { 0.0 };
        sample.proposals_published = index as u64 * 2;
        sample.directive_generation = Some(index as u64);
    }
    let stats = quiet_stats(&samples, 2_000);
    assert_eq!(stats.target_range_ev, Some(0.25));
    assert_eq!(stats.sensor_range_ev, Some(0.25));
    assert_eq!(
        stats.screen_range_ev,
        Some(0.375),
        "экран = сенсор + подтяжка: от -0,125 до 0,25"
    );
    assert_eq!(stats.proposals_published, 8);
    assert_eq!(stats.directive_generations, Some(4));
    assert_eq!(stats.proposals_per_s, 4.0);
    assert_eq!(stats.generations_per_s, Some(2.0));
}
