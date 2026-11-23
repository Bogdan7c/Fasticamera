//! Функциональные тесты сценариев видоискателя про бюджет и длительность: `viewfinder-stage-budget`,
//! `viewfinder-endurance`, `video-headroom-bench` и откат рендера при отказе камеры. Стенд (`RenderMirror`,
//! `BatteryPump`, `run_viewfinder*`) — из `event_router_viewfinder_tests`.

use super::*;

#[test]
fn stage_budget_measures_each_row_applies_stage_sets_and_restores_the_product() {
    let config = short_config(|viewfinder, _| {
        viewfinder.stage_budget.rows = vec![
            StageBudgetRowSetting::Stage(IspStageSetting::Tone),
            StageBudgetRowSetting::WholeViewfinder,
        ];
    });
    let (harness, result) = run_viewfinder(
        config,
        "viewfinder-stage-budget",
        MirrorFaults::default(),
        Some(PowerPlug::Unplugged),
    );

    assert_eq!(result["status"], "passed", "{result}");
    let table: Vec<String> = result["report"]["table"]
        .as_array()
        .expect("таблица")
        .iter()
        .map(|line| line.as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(table.len(), 3, "две строки + «камера без ISP»: {table:?}");
    assert!(table[0].starts_with("tone | 0.10 / 0.10 | 20.0"), "{table:?}");
    assert!(table[0].ends_with("| measured"), "{table:?}");
    // Весь видоискатель: кадр ISP (1 мс + 0,1 мс на стадию) + 0,6 мс против мозаики 0,7 мс.
    let whole_viewfinder_ms = (isp_micros(IspStageSet::ALL) + 600 - 700) as f64 / 1_000.0;
    let whole_viewfinder_row = format!("whole_viewfinder | {whole_viewfinder_ms:.2} / {whole_viewfinder_ms:.2} | 20.0");
    assert!(table[1].starts_with(&whole_viewfinder_row), "{table:?}");
    assert!(
        table[2].starts_with("camera_without_isp | 0.70 / 1.00 | 500.0"),
        "{table:?}"
    );

    let recorded = harness.control.recorded();
    let without_tone = IspStageSet::ALL.without(IspStage::Tone);
    assert!(
        recorded.contains(&RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(
            without_tone
        )))
    );
    assert!(
        recorded.contains(&RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(
            ViewfinderPainter::DebugMosaic
        )))
    );
    // Конец сценария возвращает продукт: рисовальщик ISP и все стадии — последними.
    let tail = &recorded[recorded.len() - 2..];
    assert_eq!(
        tail,
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(ViewfinderPainter::Isp)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(IspStageSet::ALL)),
        ]
    );
    let rows: Vec<serde_json::Value> = stages_named(&harness, "row_started")
        .iter()
        .map(|stage| stage["row"].clone())
        .collect();
    assert_eq!(rows, [serde_json::json!("tone"), serde_json::json!("whole_viewfinder")]);
    assert_fits_logcat(&harness, &result);
}

#[test]
fn stage_budget_refuses_on_external_power_without_measuring_further_rows() {
    let config = short_config(|viewfinder, _| {
        viewfinder.stage_budget.rows = vec![
            StageBudgetRowSetting::Stage(IspStageSetting::Levels),
            StageBudgetRowSetting::Stage(IspStageSetting::Tone),
        ];
    });
    let (harness, result) = run_viewfinder(
        config,
        "viewfinder-stage-budget",
        MirrorFaults::default(),
        Some(PowerPlug::Usb),
    );

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("ток не измерен"), "{error}");
    assert!(error.contains("ExternalPower"), "{error}");
    assert_eq!(
        stages_named(&harness, "row_started").len(),
        1,
        "вторую строку не начинали"
    );
    // Первое же показание на зарядке отвергает замер: продукт остаётся (или возвращён) продуктом.
    let left = effective(&harness.control.recorded());
    assert_eq!((left.painter, left.stages), (ViewfinderPainter::Isp, IspStageSet::ALL));
}

#[test]
fn stage_budget_refuses_without_a_battery_source() {
    let (_harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "viewfinder-stage-budget",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "failed", "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("опрос батареи не запущен"),
        "{result}"
    );
}

#[test]
fn endurance_records_every_minute_and_reports_no_throttling_on_a_calm_run() {
    let (harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "viewfinder-endurance",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["expected_minutes"], 3);
    assert_eq!(report["throttling"]["found"], false);
    assert!(
        report["power_unavailable"].is_string(),
        "без батареи — честно: {report}"
    );
    assert_eq!(stages_named(&harness, "minute").len(), 3);
    let isp = stages_named(&harness, "minute")[0]["isp_median_ms"]
        .as_f64()
        .expect("ISP");
    // Медиана ISP за минуту: 1 мс + 0,1 мс на каждую стадию из списка продукта.
    let expected_isp_ms = isp_micros(IspStageSet::ALL) as f64 / 1_000.0;
    assert!((isp - expected_isp_ms).abs() < 1e-9, "{isp} вместо {expected_isp_ms}");
    assert_fits_logcat(&harness, &result);
}

#[test]
fn endurance_finds_throttling_when_the_sensor_is_slower_than_the_threshold() {
    let config = short_config(|viewfinder, _| viewfinder.endurance.min_sensor_fps = 61.0);
    let (_harness, result) = run_viewfinder(config, "viewfinder-endurance", MirrorFaults::default(), None);

    assert_eq!(result["status"], "passed", "троттлинг — находка, не провал: {result}");
    assert_eq!(result["report"]["throttling"]["found"], true);
    assert_eq!(result["report"]["throttling"]["signs"][0]["sign"], "sensor_fps_drop");
}

#[test]
fn headroom_bench_runs_at_120_hz_and_end_restores_bench_off_and_60_hz() {
    let (harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "video-headroom-bench",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    // 16:9@60 основной камеры идёт публичным диапазоном AE (4:3@60 — вендорским ключом, факт 00).
    assert_eq!(report["configured"]["lever"], "public_ae_target_range", "{report}");
    assert_eq!(report["bench"]["total"]["p95_ms"], 12.3);
    assert_eq!(report["fits_frame_budget"], true);
    assert_eq!(report["display"]["refresh_hz"], 120.0);
    assert!(report["bench"]["frames_recorded"].as_u64().expect("кадры") > 0);
    let started = stages_named(&harness, "bench_started");
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0]["tap_for_refresh"], true,
        "драйвер касается экрана ради 120 Гц"
    );
    assert_eq!(*harness.shell.frame_rates.lock().expect("замок"), [120.0, 60.0]);
    assert_eq!(
        harness.control.recorded(),
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled)),
        ]
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn camera_failure_during_a_viewfinder_scenario_still_restores_the_render() {
    let (harness, result) = {
        let mut harness = harness_for_rig(FakeCameraRig::with_config(short_config(|_, _| {})));
        harness.deliver(ShellEvent::Resumed);
        // Рендер камеру не показывает вовсе: сценарий упадёт на ожидании первого кадра.
        let _pump = StreamPump::start(
            harness.rig(),
            Arc::clone(&harness.render),
            PumpFaults {
                render_shows_nothing: true,
                ..PumpFaults::default()
            },
        );
        harness.start(Scenario::parse("video-headroom-bench").expect("сценарий"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
            harness.router.step(Duration::from_millis(2));
        }
        let result = harness.output.single_result();
        (harness, result)
    };

    assert_eq!(result["status"], "failed", "{result}");
    // Стенд не включался и частоту не трогали — возвращать нечего.
    assert!(harness.control.recorded().is_empty());
    assert!(harness.shell.frame_rates.lock().expect("замок").is_empty());
}
