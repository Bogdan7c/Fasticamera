//! Сценарий `render`: схождение частоты экрана (устойчивость, стадия `awaiting_refresh`, отказ).
//! Время — настоящее (тик маршрутизатора по 2-5 мс), поэтому пороги в тестах — десятки мс.

use super::*;

/// Короткие пороги схождения, чтобы тест не ждал секунды.
fn quick_settle() -> ScenarioConfig {
    ScenarioConfig {
        render_refresh_stable_ms: 60,
        render_refresh_retry_ms: 40,
        render_refresh_settle_timeout_ms: 2_000,
        ..ScenarioConfig::default()
    }
}

fn period_of(hertz: f64) -> Duration {
    Duration::from_secs_f64(1.0 / hertz)
}

fn set_refresh_period(status: &mut render_loop::thread::RenderStatus, period: Duration) {
    status.pacing_refresh_period = period;
    status.display.as_mut().expect("display").refresh_cycle = Some(period);
}

/// Статус с заданным периодом экрана (и пейсинга).
fn render_at(hertz: f64) -> Arc<FakeRenderStatus> {
    let render = FakeRenderStatus::live(presenting_status(), ValidationState::Enabled);
    render.update(|status| set_refresh_period(status, period_of(hertz)));
    render
}

/// Все записанные стадии с данным именем (JSON-объекты вместе с полями).
fn stages_named(harness: &Harness, name: &str) -> Vec<serde_json::Value> {
    harness
        .output
        .stages
        .lock()
        .expect("замок")
        .iter()
        .filter(|stage| stage["stage"] == name)
        .cloned()
        .collect()
}

fn run_for(harness: &mut Harness, duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        harness.router.step(Duration::from_millis(2));
    }
}

#[test]
fn transient_target_then_cap_then_target_settles_only_after_the_stable_period() {
    // Цель 120 Гц. Сначала переходные 8,33 мс, затем колпак 60 Гц, затем снова 120.
    let render = render_at(120.0);
    let mut harness = harness_with_status(
        quick_settle(),
        quick_window(),
        Arc::clone(&render),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    run_for(&mut harness, Duration::from_millis(20));
    assert!(
        stages_named(&harness, "refresh_settled").is_empty(),
        "переходное значение не должно сойтись мгновенно"
    );

    render.update(|status| set_refresh_period(status, period_of(60.0)));
    run_for(&mut harness, Duration::from_millis(150));
    assert!(
        stages_named(&harness, "refresh_settled").is_empty(),
        "колпак 60 Гц не сошёлся"
    );

    render.update(|status| set_refresh_period(status, period_of(120.0)));
    run_for(&mut harness, Duration::from_millis(25));
    assert!(
        stages_named(&harness, "refresh_settled").is_empty(),
        "цель вернулась меньше устойчивого срока назад"
    );
    run_for(&mut harness, Duration::from_millis(120));
    let settled = stages_named(&harness, "refresh_settled");
    assert_eq!(settled.len(), 1, "{:?}", harness.output.stage_names());
    assert!(
        settled[0]["waited_ms"].as_f64().expect("мс") >= 150.0,
        "отсчёт устойчивости шёл с возврата цели: {}",
        settled[0]
    );
    harness.run_until_result();
    assert_eq!(harness.output.single_result()["status"], "passed");
}

#[test]
fn awaiting_refresh_repeats_with_attempt_numbers_and_taps_only_when_display_is_slower() {
    let render = render_at(60.0);
    let mut harness = harness_with_status(
        quick_settle(),
        RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            ..quick_window()
        },
        Arc::clone(&render),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    run_for(&mut harness, Duration::from_millis(200));

    let stages = stages_named(&harness, "awaiting_refresh");
    assert!(stages.len() >= 3, "стадия повторяется: {stages:?}");
    let attempts: Vec<u64> = stages
        .iter()
        .map(|stage| stage["attempt"].as_u64().expect("номер"))
        .collect();
    assert_eq!(
        attempts,
        (1..=attempts.len() as u64).collect::<Vec<_>>(),
        "попытки идут подряд"
    );
    for stage in &stages {
        assert_eq!(
            stage["observed_slower_than_expected"], true,
            "60 Гц при цели 120: касание нужно"
        );
        assert!((stage["observed_period_ms"].as_f64().expect("мс") - 16.667).abs() < 0.01);
        assert!((stage["expected_period_ms"].as_f64().expect("мс") - 8.333).abs() < 0.01);
    }

    // Экран быстрее цели (цель 120, на экране 240): касание не поможет.
    render.update(|status| set_refresh_period(status, period_of(240.0)));
    run_for(&mut harness, Duration::from_millis(100));
    let later = stages_named(&harness, "awaiting_refresh");
    let last = later.last().expect("стадия");
    assert_eq!(last["observed_slower_than_expected"], false, "{last}");
}

#[test]
fn default_sixty_hertz_target_with_sixty_hertz_display_never_asks_for_a_touch() {
    let render = render_at(60.0);
    let config = ScenarioConfig {
        render_refresh_stable_ms: 50,
        ..quick_settle()
    };
    // Конфиг рендера по умолчанию: цель 60 Гц.
    let render_config = RenderLoopConfig {
        pacing_window_s: 0.15,
        ..RenderLoopConfig::default()
    };
    assert_eq!(render_config.preferred_frame_rate_hz, 60.0);
    let mut harness = harness_with_status(config, render_config, render, FakeGpuDiagnostics::new(Vec::new(), None));
    harness.start(Scenario::Render);
    harness.run_until_result();

    let result = harness.output.single_result();
    assert_eq!(result["status"], "passed", "{result}");
    assert!((result["report"]["refresh"]["expected_period_ms"].as_f64().expect("мс") - 16.667).abs() < 0.01);
    assert_eq!(
        result["report"]["refresh"]["synthetic_touch_requested"], false,
        "касание не запрашивалось, отчёт не должен утверждать обратное"
    );
    assert!(
        stages_named(&harness, "awaiting_refresh")
            .iter()
            .all(|stage| stage["observed_slower_than_expected"] == false),
        "касание не требуется"
    );
}

#[test]
fn never_settling_refresh_fails_with_observed_and_expected_periods() {
    let config = ScenarioConfig {
        render_refresh_settle_timeout_ms: 60,
        ..quick_settle()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            ..quick_window()
        },
        render_at(60.0),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    harness.run_until_result();

    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    let error = result["error"].as_str().expect("error");
    assert!(error.contains("16.667 мс"), "наблюдаемый период в причине: {error}");
    assert!(error.contains("8.333 мс"), "ожидаемый тоже: {error}");
    assert!(error.contains("медленнее"), "направление отклонения названо: {error}");
    assert!(
        stages_named(&harness, "refresh_settled").is_empty(),
        "окно пейсинга не начиналось"
    );
}

#[test]
fn display_faster_than_target_fails_without_asking_for_a_touch() {
    // Цель 60 Гц (по умолчанию), экран 120 Гц.
    let config = ScenarioConfig {
        render_refresh_settle_timeout_ms: 60,
        ..quick_settle()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig {
            pacing_window_s: 0.15,
            ..RenderLoopConfig::default()
        },
        render_at(120.0),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    harness.run_until_result();

    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(result["error"].as_str().expect("error").contains("быстрее"), "{result}");
    assert!(
        stages_named(&harness, "awaiting_refresh")
            .iter()
            .all(|stage| stage["observed_slower_than_expected"] == false)
    );
}

#[test]
fn display_on_target_but_pacing_lagging_does_not_settle() {
    // Экран уже 120 Гц, но пейсинг ещё судит по 60 Гц: окно мерило бы не то.
    let render = render_at(60.0);
    render.update(|status| {
        status.display.as_mut().expect("display").refresh_cycle = Some(period_of(120.0));
    });
    let config = ScenarioConfig {
        render_refresh_settle_timeout_ms: 60,
        ..quick_settle()
    };
    let mut harness = harness_with_status(
        config,
        RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            ..quick_window()
        },
        render,
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    harness.run_until_result();
    assert_eq!(harness.output.single_result()["status"], "failed");
    assert!(stages_named(&harness, "refresh_settled").is_empty());
}

#[test]
fn report_says_synthetic_touch_was_requested_only_after_a_slower_display_stage() {
    // Цель 120, экран 60: сценарий просит касание, затем «касание подействовало».
    let render = render_at(60.0);
    let mut harness = harness_with_status(
        quick_settle(),
        RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            pacing_window_s: 0.15,
            ..RenderLoopConfig::default()
        },
        Arc::clone(&render),
        FakeGpuDiagnostics::new(Vec::new(), None),
    );
    harness.start(Scenario::Render);
    run_for(&mut harness, Duration::from_millis(120));
    assert!(!stages_named(&harness, "awaiting_refresh").is_empty());
    render.update(|status| set_refresh_period(status, period_of(120.0)));
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        result["report"]["refresh"]["synthetic_touch_requested"], true,
        "{result}"
    );
}
