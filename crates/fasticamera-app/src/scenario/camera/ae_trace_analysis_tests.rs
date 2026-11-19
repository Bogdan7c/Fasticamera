//! Разбор прогона `ae-trace` целиком: синтетический прогон с известным ответом по каждой фазе.

use camera_contract::three_a::ThreeAProfile;

use super::super::ae_trace_series::FlickerReading;
use super::super::ae_trace_test_support::{flat_sample, new_frame};
use super::*;

/// Короткое расписание: фазы по 4 с, подсказка каждые 2 с, окно 0,5 с, разогрев статики 1 с.
fn short_config() -> AeTraceScenarioConfig {
    AeTraceScenarioConfig {
        photo_phase_ms: 4_000,
        video_phase_ms: 4_000,
        static_phase_ms: 4_000,
        cue_interval_ms: 2_000,
        static_warmup_ms: 1_000,
        sample_interval_ms: 50,
        step_window_ms: 500,
        min_window_samples: 5,
        series_capacity: 1_000,
        ..AeTraceScenarioConfig::default()
    }
}

/// Прогон: сцена в фазах с подсказками переключается на каждой подсказке (0 -> 1 -> 0), сенсор мгновенно следует,
/// в static всё ровно. Профиль и ответ детектора заданы по фазам.
fn synthetic_run(config: &AeTraceScenarioConfig, video_profile: ThreeAProfile) -> (Timeline, Vec<TraceSample>) {
    let timeline = Timeline::from_config(config);
    let samples = (0..timeline.total_ms() / config.sample_interval_ms)
        .map(|index| {
            let t_ms = index * config.sample_interval_ms;
            let (phase_window, level) = {
                let window = timeline
                    .phases()
                    .iter()
                    .find(|window| (window.start_ms..window.end_ms).contains(&t_ms))
                    .expect("момент внутри расписания");
                let cues_passed = window.cues_ms.iter().filter(|cue| **cue <= t_ms).count();
                (window, (cues_passed % 2) as f64)
            };
            let mut sample = flat_sample(t_ms, level);
            sample.profile = match phase_window.phase {
                TracePhase::Video => video_profile,
                _ => ThreeAProfile::Photo,
            };
            sample.new_frame = new_frame(Some(10.0), FlickerReading::Mains50Hz);
            sample
        })
        .collect();
    (timeline, samples)
}

#[test]
fn run_produces_one_analysis_per_phase_with_the_expected_steps() {
    let config = short_config();
    let (timeline, samples) = synthetic_run(&config, ThreeAProfile::Video);

    let analysis = analyze_run(&config, &timeline, &samples);

    assert_eq!(analysis.samples, samples.len());
    let names: Vec<&str> = analysis.phases.iter().map(|phase| phase.phase.name()).collect();
    assert_eq!(names, vec!["photo", "video", "static"]);
    // Подсказка на 2 с — единственная в фазе в 4 с.
    for phase in [TracePhase::Photo, TracePhase::Video] {
        let result = analysis.phase(phase).expect("фаза есть");
        assert_eq!(result.steps.len(), 1, "{phase:?}");
        assert_eq!(result.count(StepKind::Measured), 1, "{phase:?}: {:?}", result.steps);
        let step = result.steps[0].step_ev.expect("ступенька");
        assert!((step - 1.0).abs() < 1e-9, "{phase:?}: {step}");
    }
    assert_eq!(analysis.measured_step_count(), 2);
}

#[test]
fn quiet_stats_exist_only_for_the_phase_without_cues_and_skip_the_warmup() {
    let config = short_config();
    let (timeline, mut samples) = synthetic_run(&config, ThreeAProfile::Video);
    // Выброс в разогреве static (первая секунда) не должен попасть в размах; выброс после разогрева — должен.
    for sample in &mut samples {
        if sample.t_ms == 8_200 {
            sample.target_ev = Some(5.0);
        }
        if sample.t_ms == 10_000 {
            sample.target_ev = Some(0.5);
        }
    }

    let analysis = analyze_run(&config, &timeline, &samples);

    assert!(analysis.phase(TracePhase::Photo).expect("фото").quiet.is_none());
    let quiet = analysis
        .phase(TracePhase::Static)
        .expect("static")
        .quiet
        .clone()
        .expect("статика");
    assert_eq!(quiet.analyzed_ms, 3_000);
    assert_eq!(
        quiet.target_range_ev,
        Some(0.5),
        "выброс 5,0 EV лежит в разогреве и не считается"
    );
}

#[test]
fn profile_confirmation_reflects_what_the_engine_actually_held() {
    let config = short_config();
    let (timeline, honest) = synthetic_run(&config, ThreeAProfile::Video);
    let (_, silently_photo) = synthetic_run(&config, ThreeAProfile::Photo);

    let confirmed = analyze_run(&config, &timeline, &honest);
    let unconfirmed = analyze_run(&config, &timeline, &silently_photo);

    assert!(confirmed.phases.iter().all(|phase| phase.profile_confirmed));
    let video = unconfirmed.phase(TracePhase::Video).expect("видео");
    assert!(
        !video.profile_confirmed,
        "движок остался на «Фото» — видео-числа были бы фото-числами"
    );
    assert!(unconfirmed.phase(TracePhase::Photo).expect("фото").profile_confirmed);
}

#[test]
fn flicker_is_counted_per_phase() {
    let config = short_config();
    let (timeline, mut samples) = synthetic_run(&config, ThreeAProfile::Video);
    for sample in samples
        .iter_mut()
        .filter(|sample| (4_000..8_000).contains(&sample.t_ms))
    {
        sample.new_frame = new_frame(Some(7.0), FlickerReading::None);
    }

    let analysis = analyze_run(&config, &timeline, &samples);

    let photo = analysis.phase(TracePhase::Photo).expect("фото");
    let video = analysis.phase(TracePhase::Video).expect("видео");
    assert_eq!(photo.flicker.readings["mains_50hz"], 80);
    assert_eq!(photo.flicker.readings["none"], 0);
    assert_eq!(video.flicker.readings["none"], 80);
    assert_eq!(video.flicker.max_exposure_ms, Some(7.0));
    assert_eq!(photo.flicker.multiple_share(), Some(1.0));
    assert_eq!(
        video.flicker.multiple_share(),
        None,
        "7 мс короче 10: длинных кадров нет"
    );
}

#[test]
fn phase_gain_and_worst_overshoot_come_from_that_phase_only() {
    let config = short_config();
    let (timeline, mut samples) = synthetic_run(&config, ThreeAProfile::Video);
    for sample in samples.iter_mut().filter(|sample| sample.t_ms == 6_000) {
        sample.gain_ev = -0.4;
    }

    let analysis = analyze_run(&config, &timeline, &samples);

    assert_eq!(analysis.max_abs_gain_ev, 0.4);
    assert_eq!(analysis.phase(TracePhase::Video).expect("видео").max_abs_gain_ev, 0.4);
    assert_eq!(analysis.phase(TracePhase::Photo).expect("фото").max_abs_gain_ev, 0.0);
    assert_eq!(
        analysis.phase(TracePhase::Photo).expect("фото").worst_overshoot_ev(),
        0.0
    );
}

#[test]
fn empty_series_yields_unjudged_steps_instead_of_panicking() {
    let config = short_config();
    let timeline = Timeline::from_config(&config);

    let analysis = analyze_run(&config, &timeline, &[]);

    assert_eq!(analysis.measured_step_count(), 0);
    for phase in &analysis.phases {
        assert!(!phase.profile_confirmed);
        assert_eq!(phase.count(StepKind::Measured), 0);
    }
    assert_eq!(
        analysis
            .phase(TracePhase::Photo)
            .expect("фото")
            .count(StepKind::InsufficientData),
        1
    );
}
