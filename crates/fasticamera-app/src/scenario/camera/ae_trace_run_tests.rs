//! Запись `ae-trace`: события расписания исполняются вовремя и один раз, профиль меняется на настоящем движке,
//! сигналы владельцу попадают в журнал с исходом платформы, ряд пишется не чаще заданного шага.

use std::time::Duration;

use camera_contract::three_a::ThreeAProfile;
use shell_contract::commands::CommandOutcome;
use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptKind};

use super::super::ae_trace_test_support::diagnostics;
use super::*;
use crate::camera_test_support::{FakeCameraRig, WAIT};
use crate::operator_prompt_fake::FakePrompts;

fn config() -> AeTraceScenarioConfig {
    AeTraceScenarioConfig {
        photo_phase_ms: 1_600,
        video_phase_ms: 1_600,
        static_phase_ms: 800,
        cue_interval_ms: 800,
        // Отсчёт 2 x 100 мс + надпись 200 мс помещаются в период 800 мс.
        cue_countdown_steps: 2,
        cue_countdown_step_ms: 100,
        cue_display_ms: 200,
        static_warmup_ms: 100,
        sample_interval_ms: 50,
        step_window_ms: 300,
        series_capacity: 1_000,
        ..AeTraceScenarioConfig::default()
    }
}

/// Один шаг записи в момент `start + at_ms`.
fn step(
    tracing: &mut Tracing,
    config: &AeTraceScenarioConfig,
    start: Instant,
    at_ms: u64,
    rig: &FakeCameraRig,
    prompts: &FakePrompts,
) -> Result<Advance, RunFailure> {
    let handle = rig.handle();
    let diag = diagnostics(Some(1.0), Some(1.0), 0.0, 0, Some(0));
    let status = handle.status();
    tracing.advance(
        config,
        start + Duration::from_millis(at_ms),
        RecordingServices {
            handle: &handle,
            prompts,
        },
        Readings {
            diagnostics: &diag,
            three_a: &status.three_a,
        },
    )
}

fn profile_becomes(rig: &FakeCameraRig, expected: ThreeAProfile) -> bool {
    rig.handle()
        .wait_until_status(WAIT, |status| status.three_a.profile == expected)
        .is_some()
}

#[test]
fn phase_starts_switch_the_engine_profile_and_the_prompt_goes_prepare_act_wait() {
    let rig = FakeCameraRig::new();
    let config = config();
    let prompts = FakePrompts::default();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    assert_eq!(
        step(&mut tracing, &config, start, 0, &rig, &prompts),
        Ok(Advance::Continue)
    );
    assert_eq!(prompts.shown().len(), 1, "старт фото");
    assert_eq!(prompts.shown()[0].headline(), "ФАЗА 1 ИЗ 3: ФОТО");
    assert!(profile_becomes(&rig, ThreeAProfile::Photo));

    step(&mut tracing, &config, start, 599, &rig, &prompts).expect("шаг");
    assert_eq!(prompts.shown().len(), 1, "до отсчёта ничего нового");

    // Отсчёт 2 -> 1 -> подсказка -> возврат «жди»: подсказка висит как состояние, каждая сменяет прежнюю.
    step(&mut tracing, &config, start, 600, &rig, &prompts).expect("шаг");
    step(&mut tracing, &config, start, 700, &rig, &prompts).expect("шаг");
    step(&mut tracing, &config, start, 800, &rig, &prompts).expect("шаг");
    step(&mut tracing, &config, start, 1_000, &rig, &prompts).expect("шаг");
    let shown = prompts.shown();
    assert_eq!(
        shown.iter().map(|prompt| prompt.headline()).collect::<Vec<_>>(),
        vec![
            "ФАЗА 1 ИЗ 3: ФОТО",
            "ПРИГОТОВЬСЯ… 2",
            "ПРИГОТОВЬСЯ… 1",
            "ЩЁЛКНИ ЛАМПОЙ",
            "ЖДИ СЛЕДУЮЩЕЙ СТУПЕНЬКИ",
        ]
    );
    assert_eq!(
        prompts.shown_kinds(),
        vec![
            OperatorPromptKind::Info,
            OperatorPromptKind::Prepare,
            OperatorPromptKind::Prepare,
            OperatorPromptKind::Act,
            OperatorPromptKind::Info,
        ]
    );

    step(&mut tracing, &config, start, 1_600, &rig, &prompts).expect("шаг");
    assert_eq!(
        prompts.shown().last().map(|prompt| prompt.headline().to_owned()),
        Some("ФАЗА 2 ИЗ 3: ВИДЕО".to_owned()),
        "старт видео"
    );
    assert!(
        profile_becomes(&rig, ThreeAProfile::Video),
        "движок получил профиль «Видео»"
    );

    // Повторный шаг в тот же момент не повторяет события.
    let before = prompts.shown().len();
    step(&mut tracing, &config, start, 1_600, &rig, &prompts).expect("шаг");
    assert_eq!(prompts.shown().len(), before);

    step(&mut tracing, &config, start, 3_200, &rig, &prompts).expect("шаг");
    assert_eq!(
        prompts.shown().last().map(|prompt| prompt.headline().to_owned()),
        Some("ФАЗА 3 ИЗ 3: СТАТИКА".to_owned()),
        "старт static"
    );
    assert!(profile_becomes(&rig, ThreeAProfile::Photo), "static снова на «Фото»");
}

#[test]
fn schedule_finishes_exactly_at_the_end_and_the_outcome_carries_the_series() {
    let rig = FakeCameraRig::new();
    let config = config();
    let prompts = FakePrompts::default();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    assert_eq!(
        step(&mut tracing, &config, start, 3_950, &rig, &prompts),
        Ok(Advance::Continue)
    );
    assert_eq!(
        step(&mut tracing, &config, start, 4_000, &rig, &prompts),
        Ok(Advance::ScheduleFinished)
    );

    let outcome = tracing.finish();
    assert_eq!(outcome.timeline.total_ms(), 4_000);
    assert_eq!(outcome.series.len(), 2, "шаги через 50 мс — два отсчёта");
    assert_eq!(outcome.series[1].t_ms, 4_000);
    // Пропущенные события догнаны одним шагом: 3 старта фаз и по 4 события (2 отсчёта, подсказка, «жди»)
    // на каждую из двух подсказок (по одной в фазах фото и видео: период 800 мс в фазе 1 600 мс).
    assert_eq!(prompts.shown().len(), 3 + 2 * 4);
    assert_eq!(outcome.prompts.queued, 11, "все принятые платформой записаны в журнал");
}

#[test]
fn cue_moments_stay_where_the_vibration_used_to_be() {
    // Подсказка «ЩЁЛКНИ ЛАМПОЙ» должна наступать ровно в момент подсказки расписания, а не сдвигаться отсчётом.
    let rig = FakeCameraRig::new();
    let config = config();
    let prompts = FakePrompts::default();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    step(&mut tracing, &config, start, 799, &rig, &prompts).expect("шаг");
    assert!(
        prompts
            .shown()
            .iter()
            .all(|prompt| prompt.kind() != OperatorPromptKind::Act),
        "за миллисекунду до момента подсказки её ещё нет"
    );
    step(&mut tracing, &config, start, 800, &rig, &prompts).expect("шаг");
    assert_eq!(
        prompts.shown().last().map(OperatorPrompt::kind),
        Some(OperatorPromptKind::Act)
    );
    assert_eq!(config.cue_interval_ms, 800);
}

#[test]
fn samples_are_written_no_more_often_than_the_configured_interval() {
    let rig = FakeCameraRig::new();
    let config = config();
    let prompts = FakePrompts::default();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    for at_ms in (0..=200).step_by(10) {
        step(&mut tracing, &config, start, at_ms, &rig, &prompts).expect("шаг");
    }

    let times: Vec<u64> = tracing.finish().series.iter().map(|sample| sample.t_ms).collect();
    assert_eq!(times, vec![0, 50, 100, 150, 200]);
}

#[test]
fn undelivered_prompts_are_recorded_with_the_platform_outcome() {
    let rig = FakeCameraRig::new();
    let config = config();
    let prompts = FakePrompts::default();
    prompts.set_outcome(Some(CommandOutcome::NotAttached));
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    step(&mut tracing, &config, start, 800, &rig, &prompts).expect("шаг: подсказка не доставлена, но прогон идёт");

    let outcome = tracing.finish();
    assert_eq!(outcome.prompts.queued, 0);
    // Старт фото, два шага отсчёта и сама подсказка.
    assert_eq!(outcome.prompts.failures.len(), 4, "{:?}", outcome.prompts.failures);
    assert!(
        outcome.prompts.failures[3].contains("подсказка 1 фазы photo"),
        "{:?}",
        outcome.prompts.failures
    );
    assert!(outcome.prompts.failures[3].contains("NotAttached"));
}

#[test]
fn ticks_without_diagnostics_are_counted_and_still_end_the_schedule() {
    let config = config();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    assert_eq!(
        tracing.note_missing_diagnostics(start + Duration::from_millis(10)),
        Advance::Continue
    );
    assert_eq!(
        tracing.note_missing_diagnostics(start + Duration::from_millis(4_000)),
        Advance::ScheduleFinished,
        "расписание не должно зависнуть, если диагностика пропала на последних тиках"
    );
    assert_eq!(tracing.finish().ticks_without_ae, 2);
}
