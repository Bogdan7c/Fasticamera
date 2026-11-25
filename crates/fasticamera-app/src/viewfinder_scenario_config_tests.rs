use isp_vulkan::viewfinder::IspStage;

use super::*;
use crate::runtime_config::RuntimeConfig;

#[test]
fn defaults_are_valid_and_cover_every_stage_plus_whole_viewfinder() {
    let config = ViewfinderScenarioConfig::default();
    config.validate().expect("значения по умолчанию верны");
    let rows = &config.stage_budget.rows;
    assert_eq!(
        rows.len(),
        IspStage::ALL.len() + 1,
        "все стадии + весь видоискатель: {rows:?}"
    );
    assert_eq!(rows.last(), Some(&StageBudgetRowSetting::WholeViewfinder));
    let stages: Vec<IspStage> = rows
        .iter()
        .filter_map(|row| match row {
            StageBudgetRowSetting::Stage(stage) => Some(stage.to_stage()),
            _ => None,
        })
        .collect();
    assert_eq!(stages, IspStage::ALL, "порядок строк — порядок прохода");
    assert_eq!(config.smoke.isp_dispatch_budget_ms, 3.0, "цель плана 07 — меньше 3 мс");
    assert!((config.headroom.frame_budget_ms() - 16.67).abs() < 0.01);
}

#[test]
fn product_and_headroom_modes_resolve_to_main_camera_four_by_three_and_sixteen_by_nine() {
    use camera_contract::ids::{CameraTarget, LensRole};
    use camera_contract::stream::ReadoutAspect;
    let config = ViewfinderScenarioConfig::default();
    let product = config.product_step().expect("режим продукта");
    let headroom = config.headroom_step().expect("режим стенда");
    assert_eq!(product.target, CameraTarget::Role(LensRole::Wide));
    assert_eq!(product.mode.readout, ReadoutAspect::FourByThree);
    assert_eq!(headroom.mode.readout, ReadoutAspect::SixteenByNine);
    assert_eq!(headroom.mode.frame_rate.frames_per_second(), 60);
}

#[test]
fn rows_are_named_in_json_and_unknown_or_duplicate_rows_are_rejected() {
    let parsed: StageBudgetConfig = serde_json::from_str(r#"{"rows": ["tone", "whole_viewfinder"]}"#).expect("json");
    assert_eq!(
        parsed.rows,
        [
            StageBudgetRowSetting::Stage(IspStageSetting::Tone),
            StageBudgetRowSetting::WholeViewfinder
        ]
    );
    assert_eq!(
        serde_json::to_value(&parsed.rows).expect("json"),
        serde_json::json!(["tone", "whole_viewfinder"])
    );
    assert!(serde_json::from_str::<StageBudgetConfig>(r#"{"rows": ["tonee"]}"#).is_err());

    let mut config = ViewfinderScenarioConfig::default();
    config.stage_budget.rows = vec![
        StageBudgetRowSetting::WholeViewfinder,
        StageBudgetRowSetting::WholeViewfinder,
    ];
    assert_eq!(
        config.validate(),
        Err(ViewfinderScenarioConfigError::DuplicateStageRow {
            name: "whole_viewfinder"
        })
    );
    config.stage_budget.rows.clear();
    assert_eq!(config.validate(), Err(ViewfinderScenarioConfigError::NoStageRows));
}

#[test]
fn intent_subset_arrives_through_runtime_config_like_kotlin_sends_it() {
    // Ровно та форма, что собирает NativeConfig.kt при `-e stages levels,whole_viewfinder`.
    let json = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{"rows":["levels","whole_viewfinder"]}}}}}"#;
    let runtime = RuntimeConfig::from_json(json).expect("частичная секция принимается");
    assert_eq!(
        runtime.camera().scenarios.viewfinder.stage_budget.rows,
        [
            StageBudgetRowSetting::Stage(IspStageSetting::Levels),
            StageBudgetRowSetting::WholeViewfinder
        ]
    );
    // Остальное — по умолчанию.
    assert_eq!(
        runtime.camera().scenarios.viewfinder.smoke,
        ViewfinderSmokeConfig::default()
    );

    let typo = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{"rows":["levls"]}}}}}"#;
    assert!(RuntimeConfig::from_json(typo).is_err(), "опечатка видна при initialize");
    let empty = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{"rows":[]}}}}}"#;
    assert!(RuntimeConfig::from_json(empty).is_err(), "пустой список — тоже ошибка");
}

#[test]
fn short_ab_window_arrives_next_to_stage_rows_like_kotlin_sends_it() {
    // Форма NativeConfig.kt при `-e stages levels -e ab_window_ms 15000`: обе частичные секции рядом.
    let json = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{"rows":["levels"]}},
        "power":{"ab":{"window_ms":15000}}}}}"#;
    let runtime = RuntimeConfig::from_json(json).expect("обе частичные секции принимаются");
    let power = &runtime.camera().scenarios.power;
    assert_eq!(power.ab.window_ms, 15_000);
    // Прогрев и число пар остались по умолчанию: окно короче, а методика та же (ABBA, ≥ 3 пары).
    let defaults = crate::power_measurement::PowerMeasurementConfig::default();
    assert_eq!(power.ab.warmup_ms, defaults.ab.warmup_ms);
    assert_eq!(power.ab.pairs, defaults.ab.pairs);
    assert_eq!(
        runtime.camera().scenarios.viewfinder.stage_budget.rows,
        [StageBudgetRowSetting::Stage(IspStageSetting::Levels)]
    );

    // Окно не длиннее прогрева — ошибка при initialize, а не бессмысленный замер.
    let too_short = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"power":{"ab":{"window_ms":5000}}}}}"#;
    assert!(RuntimeConfig::from_json(too_short).is_err());
}

#[test]
fn zero_nan_and_out_of_range_values_are_rejected() {
    let base = ViewfinderScenarioConfig::default();
    let mut zero_window = base.clone();
    zero_window.latency.window_ms = 0;
    assert_eq!(
        zero_window.validate(),
        Err(ViewfinderScenarioConfigError::Zero {
            field: "latency.window_ms"
        })
    );
    let mut nan_budget = base.clone();
    nan_budget.smoke.isp_dispatch_budget_ms = f64::NAN;
    assert!(matches!(
        nan_budget.validate(),
        Err(ViewfinderScenarioConfigError::NotPositive {
            field: "smoke.isp_dispatch_budget_ms",
            ..
        })
    ));
    let mut no_drift_allowance = base.clone();
    no_drift_allowance.smoke.max_jitter_overflow_per_minute = 0.0;
    assert!(matches!(
        no_drift_allowance.validate(),
        Err(ViewfinderScenarioConfigError::NotPositive {
            field: "smoke.max_jitter_overflow_per_minute",
            ..
        })
    ));
    let mut tolerance = base.clone();
    tolerance.smoke.frame_rate_tolerance = 1.0;
    assert!(matches!(
        tolerance.validate(),
        Err(ViewfinderScenarioConfigError::ToleranceOutOfRange { .. })
    ));
    let mut pairs = base.clone();
    pairs.latency.pairs = 2;
    assert_eq!(
        pairs.validate(),
        Err(ViewfinderScenarioConfigError::TooFewLatencyPairs { pairs: 2 })
    );
    // Верхняя граница: разности пар и нарушения окон идут в строку результата (4000 байт).
    let mut many_pairs = base.clone();
    many_pairs.latency.pairs = MAX_LATENCY_PAIRS;
    assert_eq!(many_pairs.validate(), Ok(()));
    many_pairs.latency.pairs = MAX_LATENCY_PAIRS + 1;
    assert_eq!(
        many_pairs.validate(),
        Err(ViewfinderScenarioConfigError::TooManyLatencyPairs {
            pairs: MAX_LATENCY_PAIRS + 1
        })
    );
    let mut warmup = base.clone();
    warmup.latency.warmup_ms = warmup.latency.window_ms;
    assert!(matches!(
        warmup.validate(),
        Err(ViewfinderScenarioConfigError::WarmupNotShorterThanWindow { .. })
    ));
    let mut one_profile = base.clone();
    one_profile.latency.profiles.truncate(1);
    assert_eq!(
        one_profile.validate(),
        Err(ViewfinderScenarioConfigError::LatencyProfiles(
            crate::viewfinder_latency_profile::LatencyProfilesError::WrongCount { count: 1 }
        ))
    );
    let mut same_names = base.clone();
    same_names.latency.profiles[1].name = same_names.latency.profiles[0].name.clone();
    assert!(matches!(
        same_names.validate(),
        Err(ViewfinderScenarioConfigError::LatencyProfiles(
            crate::viewfinder_latency_profile::LatencyProfilesError::DuplicateName { .. }
        ))
    ));
    let mut interval = base.clone();
    interval.headroom.snapshot_interval_ms = interval.headroom.duration_ms + 1;
    assert_eq!(
        interval.validate(),
        Err(ViewfinderScenarioConfigError::HeadroomIntervalLongerThanRun)
    );
    let mut rate = base;
    rate.headroom.frame_rate = 0;
    assert!(matches!(
        rate.validate(),
        Err(ViewfinderScenarioConfigError::InvalidFrameRate(_))
    ));
}

#[test]
fn statistics_publish_fraction_default_is_half_and_values_outside_zero_one_are_rejected() {
    assert_eq!(ViewfinderSmokeConfig::default().min_statistics_publish_fraction, 0.5);
    for bad in [0.0, -0.1, 1.5, f64::NAN] {
        let mut config = ViewfinderScenarioConfig::default();
        config.smoke.min_statistics_publish_fraction = bad;
        assert!(config.validate().is_err(), "{bad} должно отвергаться");
    }
    let mut config = ViewfinderScenarioConfig::default();
    config.smoke.min_statistics_publish_fraction = 1.0;
    config.validate().expect("1.0 — граница допустима");
}
