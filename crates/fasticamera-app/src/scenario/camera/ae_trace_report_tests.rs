//! Отчёт `ae-trace`: форма сводки, прореживание ряда, легенда и — главное — влезает ли худший случай в строку
//! результата (4000 байт logcat) вместе с текстом нарушений.

use camera_contract::three_a::ThreeAProfile;

use super::super::ae_trace_analysis::analyze_run;
use super::super::ae_trace_plan::TracePhase;
use super::super::ae_trace_series::FlickerReading;
use super::super::ae_trace_steps::{StepMetrics, StepResult};
use super::super::ae_trace_test_support::{flat_sample, new_frame};
use super::super::operator_prompt_log::PromptLog;
use super::super::result_budget::{judge_within_budget, worst_case};
use super::*;

fn short_config() -> AeTraceScenarioConfig {
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

/// Наблюдения короткого прогона: в фото ступенька есть, в видео владелец её пропустил.
fn observation(config: &AeTraceScenarioConfig) -> AeTraceObservation {
    let timeline = Timeline::from_config(config);
    let series: Vec<TraceSample> = (0..240)
        .map(|index| {
            let t_ms = index * 50;
            let level = if (2_000..4_000).contains(&t_ms) { 1.0 } else { 0.0 };
            let phase = timeline
                .phases()
                .iter()
                .find(|window| (window.start_ms..window.end_ms).contains(&t_ms))
                .map(|window| window.phase)
                .unwrap_or(TracePhase::Static);
            TraceSample {
                profile: phase.profile(),
                new_frame: new_frame(Some(10.0), FlickerReading::Mains50Hz),
                latest_flicker: Some(FlickerReading::Mains50Hz),
                ..flat_sample(t_ms, level)
            }
        })
        .collect();
    AeTraceObservation {
        gain_limit_ev: 0.5,
        analysis: Some(analyze_run(config, &timeline, &series)),
        timeline: Some(timeline),
        series,
        ..AeTraceObservation::default()
    }
}

#[test]
fn summary_has_one_row_per_cue_with_the_documented_columns() {
    let config = short_config();
    let report = summary(&observation(&config));

    assert_eq!(report["real_steps"], 1);
    assert_eq!(report["step_cols"].as_array().map(Vec::len), Some(STEP_COLUMNS.len()));
    let phases = report["phases"].as_array().expect("фазы");
    assert_eq!(phases.len(), 3);
    let photo_row = &phases[0]["steps"][0];
    assert_eq!(
        photo_row.as_array().map(Vec::len),
        Some(STEP_COLUMNS.len()),
        "{photo_row}"
    );
    assert_eq!(photo_row[0], 1, "номер подсказки с единицы");
    assert_eq!(photo_row[1], "measured");
    assert_eq!(photo_row[2], 1.0, "размер ступеньки в EV");
    let video_row = &phases[1]["steps"][0];
    assert_eq!(video_row[1], "no_step", "{video_row}");
    assert!(
        video_row[4].is_null() && video_row[5].is_null(),
        "у пропущенной ступеньки метрик нет: {video_row}"
    );
}

#[test]
fn static_phase_reports_quiet_stats_and_no_steps() {
    let config = short_config();
    let report = summary(&observation(&config));
    let static_phase = &report["phases"][2];
    assert_eq!(static_phase["phase"], "static");
    assert!(static_phase.get("steps").is_none());
    assert_eq!(static_phase["quiet"]["analyzed_ms"], 3_000);
    assert_eq!(static_phase["quiet"]["target_range_ev"], 0.0);
    assert_eq!(static_phase["flicker"]["readings"]["mains_50hz"], 80);
}

#[test]
fn run_without_analysis_still_reports_the_harness_facts() {
    let observation = AeTraceObservation {
        failure: Some("x".to_owned()),
        gain_limit_ev: 0.5,
        prompts: PromptLog {
            queued: 2,
            failures: vec!["подсказка: NotAttached".to_owned()],
        },
        ..AeTraceObservation::default()
    };
    let report = summary(&observation);
    assert_eq!(report["real_steps"], 0);
    assert!(report.get("phases").is_none());
    assert_eq!(report["prompts"]["queued"], 2);
    assert_eq!(report["prompts"]["failed"]["count"], 1);
}

#[test]
fn decimation_keeps_the_first_sample_of_each_interval() {
    let samples: Vec<TraceSample> = (0..30).map(|index| flat_sample(index * 50, 0.0)).collect();
    let kept: Vec<u64> = decimate(&samples, 250).iter().map(|sample| sample.t_ms).collect();
    assert_eq!(kept, vec![0, 250, 500, 750, 1_000, 1_250]);
    // Тик, пропустивший интервал, не размножает отсчёты: берётся первый после пропуска.
    let gappy = vec![
        flat_sample(0, 0.0),
        flat_sample(40, 0.0),
        flat_sample(900, 0.0),
        flat_sample(920, 0.0),
    ];
    let kept: Vec<u64> = decimate(&gappy, 250).iter().map(|sample| sample.t_ms).collect();
    assert_eq!(kept, vec![0, 900]);
}

#[test]
fn details_start_with_a_legend_followed_by_rounded_rows() {
    let config = short_config();
    let mut observation = observation(&config);
    observation.series[5].observed_ev = Some(0.123_456);
    observation.series[5].gain_ev = -0.049_9;
    observation.series[5].t_ms = 250;
    let items = detail_items(&config, &observation);

    let legend = &items[0]["legend"];
    assert_eq!(legend["columns"].as_array().map(Vec::len), Some(8));
    assert_eq!(legend["columns"][7], "shown_ev");
    assert_eq!(legend["phases"][0]["cues_ms"][0], 2_000);
    assert_eq!(legend["flicker"]["2"], "mains_50hz");
    assert_eq!(legend["convergence"]["3"], "holding");
    assert_eq!(
        items.len(),
        1 + 4_000 * 3 / 250 + 1 - 1,
        "легенда плюс по строке на 250 мс"
    );
    let row = items
        .iter()
        .skip(1)
        .find(|row| row[0] == 250)
        .expect("строка на 250 мс");
    assert_eq!(row[1], 0.123, "округление до тысячных");
    assert_eq!(row[4], -0.05);
    assert_eq!(row[6], 2, "код мерцания 50 Гц");
    assert_eq!(row[7], 0.0, "уровень показанного кадра из ряда");
}

/// Худший по размеру ступенька: все метрики есть и многозначные.
fn worst_step(index: usize) -> StepResult {
    StepResult {
        cue_index: index,
        cue_ms: 0,
        kind: super::super::ae_trace_steps::StepKind::Measured,
        step_ev: Some(-1.234_567),
        metrics: Some(StepMetrics {
            onset_ms: Some(12_345),
            sensor_settle_ms: Some(54_321),
            screen_settle_ms: Some(54_321),
            overshoot_ev: 0.123_456,
            sensor_overshoot_ev: 0.123_456,
            max_abs_gain_ev: -0.512_345,
            gain_at_end_ev: Some(-0.112_345),
            proposals_published: 12_345,
            directive_generations: Some(12_345),
        }),
    }
}

#[test]
fn worst_case_summary_with_long_violations_still_fits_the_logcat_record() {
    let config = AeTraceScenarioConfig::default();
    let mut observation = observation(&config);
    let mut analysis = observation.analysis.take().expect("разбор");
    for phase in analysis.phases.iter_mut().filter(|phase| phase.phase.has_cues()) {
        phase.steps = (0..crate::ae_trace_scenario_config::MAX_CUES_PER_PHASE as usize)
            .map(worst_step)
            .collect();
        phase.profile = ThreeAProfile::Video;
    }
    observation.analysis = Some(analysis);
    observation.prompts.failures = (0..20)
        .map(|n| worst_case::long_reason(&format!("подсказка {n}")))
        .collect();
    observation.restore_failure = Some(worst_case::long_reason("возврат"));
    let violations: Vec<String> = (0..12)
        .map(|n| worst_case::long_reason(&format!("нарушение {n}")))
        .collect();

    let verdict = judge_within_budget(summary(&observation), violations);

    let parsed = worst_case::assert_failure_line_fits("ae-trace", verdict.outcome);
    assert_eq!(parsed["report"]["phases"][0]["steps"].as_array().map(Vec::len), Some(6));
}
