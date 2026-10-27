//! Тесты отчётов ADPF на подделке службы: жизненный цикл сессии, отчёт с поздним GPU, отказы,
//! команды и то, что выключенный ADPF ничего не трогает.

use shell_contract::performance_hint::HintThreadId;
use shell_contract::performance_hint::fake::{FakeHintBehavior, FakePerformanceHints};

use super::*;

const PERIOD: Duration = Duration::from_nanos(16_666_667);
const RENDER_TID: i32 = 4242;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn reporter(mode: AdpfMode, fake: &FakePerformanceHints) -> AdpfReporter {
    AdpfReporter::new(
        AdpfConfig {
            mode,
            target_fraction: 0.75,
            gpu_report_wait_frames: 3,
        },
        Arc::new(fake.clone()),
    )
}

/// Кадр `index`: начало через `index` периодов, CPU 2 мс.
fn frame(index: u64) -> PresentedFrameWork {
    PresentedFrameWork {
        frame_index: index,
        period_start: MonotonicNanos::from_nanos(1_000_000_000 + i64::try_from(index).expect("индекс") * 16_666_667),
        cpu: ms(2),
    }
}

fn gpu(index: u64, value: Duration) -> FrameGpuTiming {
    FrameGpuTiming {
        frame_index: index,
        gpu_duration: value,
    }
}

#[test]
fn disabled_adpf_creates_no_session_and_sends_nothing() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Disabled, &fake);
    for index in 0..10 {
        adpf.frame_presented(frame(index), PERIOD);
        adpf.gpu_timing(gpu(index, ms(9)));
    }
    assert_eq!(fake.journal().create_attempts, 0);
    let snapshot = adpf.snapshot();
    assert_eq!(snapshot.session, AdpfSessionState::Disabled);
    assert_eq!(snapshot.reports_sent, 0);
}

#[test]
fn session_is_created_once_for_the_render_thread_with_target_fraction_of_period() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    assert_eq!(
        adpf.snapshot().session,
        AdpfSessionState::Pending,
        "до первого кадра — ждёт"
    );
    for index in 0..5 {
        adpf.frame_presented(frame(index), PERIOD);
    }
    let journal = fake.journal();
    assert_eq!(journal.sessions_created, vec![vec![HintThreadId::from_raw(RENDER_TID)]]);
    assert_eq!(journal.initial_targets, vec![PERIOD.mul_f64(0.75)]);
    let snapshot = adpf.snapshot();
    assert_eq!(snapshot.session, AdpfSessionState::Active);
    assert_eq!(snapshot.sessions_created, 1);
    assert_eq!(snapshot.target, Some(PERIOD.mul_f64(0.75)));
}

#[test]
fn late_gpu_time_is_reported_together_with_the_frame_cpu() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(frame(0), PERIOD);
    adpf.frame_presented(frame(1), PERIOD);
    assert!(fake.journal().reports.is_empty(), "кадры ждут время GPU");
    // GPU кадра 0 пришёл через два кадра.
    adpf.gpu_timing(gpu(0, ms(9)));
    let reports = fake.journal().reports;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].period_start(), frame(0).period_start);
    assert_eq!(reports[0].cpu(), ms(2));
    assert_eq!(reports[0].gpu(), Some(ms(9)));
    assert_eq!(reports[0].total(), ms(11), "общая = CPU + GPU");
    assert_eq!(adpf.snapshot().reports_with_gpu, 1);
}

#[test]
fn frame_without_gpu_is_reported_cpu_only_after_the_wait_or_when_a_later_gpu_arrives() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    // Ожидание 3 кадра: кадр 0 без GPU уходит «только CPU», когда показан кадр 3.
    for index in 0..3 {
        adpf.frame_presented(frame(index), PERIOD);
    }
    assert!(fake.journal().reports.is_empty());
    adpf.frame_presented(frame(3), PERIOD);
    let reports = fake.journal().reports;
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].period_start(), reports[0].gpu()),
        (frame(0).period_start, None)
    );
    // GPU кадра 2 пришёл: кадр 1 своего GPU уже не получит — уходит только по CPU, кадр 2 — с GPU.
    adpf.gpu_timing(gpu(2, ms(7)));
    let reports = fake.journal().reports;
    assert_eq!(reports.len(), 3);
    assert_eq!(reports[1].gpu(), None);
    assert_eq!(reports[2].gpu(), Some(ms(7)));
    let snapshot = adpf.snapshot();
    assert_eq!(
        (
            snapshot.reports_sent,
            snapshot.reports_cpu_only,
            snapshot.reports_with_gpu
        ),
        (3, 2, 1)
    );
}

#[test]
fn gpu_time_of_a_frame_that_was_never_presented_changes_nothing() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(frame(5), PERIOD);
    adpf.gpu_timing(gpu(4, ms(9)));
    assert!(fake.journal().reports.is_empty(), "кадр 5 ждёт свой GPU");
    adpf.gpu_timing(gpu(5, ms(9)));
    assert_eq!(fake.journal().reports.len(), 1);
}

#[test]
fn unsupported_platform_is_an_honest_state_without_retries() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    fake.set_behavior(FakeHintBehavior {
        create_error: Some(PerformanceHintError::Unsupported {
            reason: "createSession вернул null".to_owned(),
        }),
        ..FakeHintBehavior::default()
    });
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    for index in 0..20 {
        adpf.frame_presented(frame(index), PERIOD);
        adpf.gpu_timing(gpu(index, ms(9)));
    }
    let snapshot = adpf.snapshot();
    assert_eq!(snapshot.session, AdpfSessionState::Unsupported);
    assert_eq!(fake.journal().create_attempts, 1, "без шторма попыток на каждом кадре");
    assert_eq!(snapshot.session_create_failures, 1);
    assert_eq!(snapshot.reports_sent, 0);
    assert!(
        snapshot
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("createSession вернул null"))
    );
    // Новая команда включения — новая попытка (теперь платформа согласна).
    fake.set_behavior(FakeHintBehavior::default());
    assert_eq!(
        adpf.apply_command(AdpfCommand::Set {
            mode: AdpfMode::Enabled
        }),
        AdpfCommandOutcome::Applied
    );
    adpf.frame_presented(frame(20), PERIOD);
    assert_eq!(adpf.snapshot().session, AdpfSessionState::Active);
    assert_eq!(fake.journal().create_attempts, 2);
}

#[test]
fn creation_failure_other_than_unsupported_is_reported_as_failed() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    fake.set_behavior(FakeHintBehavior {
        create_error: Some(PerformanceHintError::Failed {
            operation: "APerformanceHint_createSession",
            code: 32,
        }),
        ..FakeHintBehavior::default()
    });
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(frame(0), PERIOD);
    assert_eq!(adpf.snapshot().session, AdpfSessionState::Failed);
}

#[test]
fn rejected_reports_are_counted_and_the_session_keeps_working() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(frame(0), PERIOD);
    fake.set_behavior(FakeHintBehavior {
        report_error: Some(PerformanceHintError::Failed {
            operation: "APerformanceHint_reportActualWorkDuration2",
            code: 32,
        }),
        ..FakeHintBehavior::default()
    });
    adpf.gpu_timing(gpu(0, ms(9)));
    let failed = adpf.snapshot();
    assert_eq!((failed.report_failures, failed.reports_sent), (1, 0));
    assert_eq!(failed.session, AdpfSessionState::Active);
    fake.set_behavior(FakeHintBehavior::default());
    adpf.frame_presented(frame(1), PERIOD);
    adpf.gpu_timing(gpu(1, ms(9)));
    assert_eq!(adpf.snapshot().reports_sent, 1);
}

#[test]
fn frame_with_zero_work_is_not_sent_but_counted() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(
        PresentedFrameWork {
            cpu: Duration::ZERO,
            ..frame(0)
        },
        PERIOD,
    );
    adpf.gpu_timing(gpu(0, Duration::ZERO));
    assert!(fake.journal().reports.is_empty());
    assert_eq!(adpf.snapshot().invalid_frames, 1);
}

#[test]
fn refresh_period_change_updates_the_target_but_jitter_does_not() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(frame(0), PERIOD);
    adpf.frame_presented(frame(1), PERIOD + Duration::from_micros(5));
    assert!(fake.journal().target_updates.is_empty(), "дрожание периода — не повод");
    let period_120 = Duration::from_nanos(8_333_333);
    adpf.frame_presented(frame(2), period_120);
    assert_eq!(fake.journal().target_updates, vec![period_120.mul_f64(0.75)]);
    assert_eq!(adpf.snapshot().target, Some(period_120.mul_f64(0.75)));
}

#[test]
fn disable_command_closes_the_session_and_enable_creates_a_new_one() {
    let fake = FakePerformanceHints::new(RENDER_TID);
    let mut adpf = reporter(AdpfMode::Enabled, &fake);
    adpf.frame_presented(frame(0), PERIOD);
    assert_eq!(
        adpf.apply_command(AdpfCommand::Set {
            mode: AdpfMode::Disabled
        }),
        AdpfCommandOutcome::Applied
    );
    assert_eq!(
        fake.journal().sessions_closed,
        1,
        "выключение закрывает сессию у системы"
    );
    adpf.gpu_timing(gpu(0, ms(9)));
    assert!(
        fake.journal().reports.is_empty(),
        "ждущий кадр выключенной фазы не отправляется"
    );
    assert_eq!(
        adpf.apply_command(AdpfCommand::Set {
            mode: AdpfMode::Disabled
        }),
        AdpfCommandOutcome::AlreadyInEffect
    );
    assert_eq!(
        adpf.apply_command(AdpfCommand::Set {
            mode: AdpfMode::Enabled
        }),
        AdpfCommandOutcome::Applied
    );
    adpf.frame_presented(frame(1), PERIOD);
    let snapshot = adpf.snapshot();
    assert_eq!(
        (snapshot.session, snapshot.sessions_created),
        (AdpfSessionState::Active, 2)
    );
    assert_eq!(
        adpf.apply_command(AdpfCommand::Set {
            mode: AdpfMode::Enabled
        }),
        AdpfCommandOutcome::AlreadyInEffect,
        "действующая сессия: повторное включение ничего не меняет"
    );
    assert_eq!(fake.journal().create_attempts, 2);
}

#[test]
fn cpu_work_excludes_acquire_and_latch_waits() {
    let started = MonotonicNanos::from_nanos(1_000_000);
    let finished = MonotonicNanos::from_nanos(1_000_000 + 10_000_000);
    let waits = FrameCpuTiming {
        acquire_wait: ms(3),
        latch_wait: ms(4),
        latch_result: crate::backend::LatchWaitResult::Waited,
        latch_to_submit: Some(ms(2)),
    };
    let measured = PresentedFrameWork::measure(7, Some(started), Some(finished), Some(&waits)).expect("замер");
    assert_eq!(
        measured.cpu,
        ms(3),
        "10 мс в кадре − 3 мс acquire − 4 мс ожидания захвата"
    );
    assert_eq!((measured.frame_index, measured.period_start), (7, started));
    let no_waits = PresentedFrameWork::measure(7, Some(started), Some(finished), None).expect("замер");
    assert_eq!(no_waits.cpu, ms(10));
    assert_eq!(
        PresentedFrameWork::measure(7, None, Some(finished), None),
        None,
        "часы не прочитались"
    );
    assert_eq!(
        PresentedFrameWork::measure(7, Some(finished), Some(started), None),
        None,
        "часы назад — замера нет"
    );
}

#[test]
fn command_log_counts_outcomes() {
    let mut log = AdpfCommandLog::default();
    let enable = AdpfCommand::Set {
        mode: AdpfMode::Enabled,
    };
    log.note(enable, AdpfCommandOutcome::Applied);
    log.note(enable, AdpfCommandOutcome::AlreadyInEffect);
    assert_eq!((log.applied, log.already_in_effect), (1, 1));
    assert_eq!(log.last, Some((enable, AdpfCommandOutcome::AlreadyInEffect)));
    let parsed: AdpfCommand = serde_json::from_str(r#"{"kind": "set", "mode": "disabled"}"#).expect("json");
    assert_eq!(
        parsed,
        AdpfCommand::Set {
            mode: AdpfMode::Disabled
        }
    );
}
