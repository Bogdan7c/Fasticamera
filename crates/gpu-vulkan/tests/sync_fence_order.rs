//! Сквозные тесты диагностики порядка fence: работа A -> sync_fd -> работа B на реальном GPU.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::sync::diagnostics::{FenceOrderParams, WorkBDependency, busy_loop_expected_value, fence_order_check};

/// Главный тест: B получила именно итог A и стартовала после конца A по меткам GPU.
#[test]
fn work_b_waits_for_exported_fence_and_reads_work_a_result() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let params = FenceOrderParams::default();

    let report = fence_order_check(&context, params).unwrap();

    let expected = busy_loop_expected_value(params.busy_seed, params.busy_iterations);
    assert_eq!(report.expected_value, expected);
    assert_eq!(report.work_a_value, expected, "A записала не тот итог: {report:?}");
    assert_eq!(report.work_b_value, expected, "B прочитала не итог A: {report:?}");
    assert!(report.b_started_after_a_finished, "B началась до конца A: {report:?}");
    assert!(report.gap_between_a_end_and_b_begin_ns >= 0, "{report:?}");
    // Это настоящий незавершённый sync_fd, а не вырожденный «уже сигнален».
    assert!(!report.exported_fence_was_already_signaled, "{report:?}");
    assert_ne!(report.release_fence_pending_at_export, Some(false), "{report:?}");
    // A длится заметно дольше погрешности меток; иначе проверка порядка ничего не доказывала бы.
    assert!(
        report.work_a_duration_ns >= MIN_WORK_A_NS,
        "A слишком короткая: {report:?}"
    );

    // Фаза 2: fence «уже сигнален» -> семафор не создан, работа B прошла и скопировала итог.
    let phase = report.already_signaled_phase;
    assert!(!phase.wait_semaphore_created, "{report:?}");
    assert!(phase.work_completed, "{report:?}");
    assert_eq!(phase.result_value, expected, "{report:?}");

    assert_eq!(context.live_objects(), baseline, "утечка объектов Vulkan");
    assert_no_validation_errors(&context);
}

/// Минимальная длительность A, нс: 5 мс (на RADV 780M по умолчанию A занимает ~14 мс).
const MIN_WORK_A_NS: u64 = 5_000_000;

/// Параметр длины работы действительно управляет длительностью A (а не игнорируется).
#[test]
fn busy_iterations_parameter_controls_work_a_duration() {
    let Some(context) = gpu_context_or_skip() else { return };
    let default_iterations = FenceOrderParams::default().busy_iterations;
    let short = fence_order_check(
        &context,
        FenceOrderParams {
            busy_iterations: default_iterations / 8,
            ..FenceOrderParams::default()
        },
    )
    .unwrap();
    let long = fence_order_check(
        &context,
        FenceOrderParams {
            busy_iterations: default_iterations,
            ..FenceOrderParams::default()
        },
    )
    .unwrap();
    assert!(
        long.work_a_duration_ns > short.work_a_duration_ns * 2,
        "короткая {} нс, длинная {} нс",
        short.work_a_duration_ns,
        long.work_a_duration_ns
    );
    assert_eq!(short.work_b_value, short.expected_value);
    assert_no_validation_errors(&context);
}

/// Повторные прогоны не копят объекты и не дают validation-ошибок (нет утечек fd/семафоров).
#[test]
fn repeated_runs_do_not_grow_live_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    for _ in 0..10 {
        let report = fence_order_check(&context, FenceOrderParams::default()).unwrap();
        assert_eq!(report.work_b_value, report.expected_value);
        assert!(report.b_started_after_a_finished);
        assert_eq!(context.live_objects(), baseline);
    }
    assert_no_validation_errors(&context);
}

/// Контрольный прогон без зависимости: диагностика не падает и корректно сообщает режим.
///
/// Честное ограничение: очередь одна (graphics+compute), драйвер RADV исполняет сабмиты по
/// порядку и без семафора, поэтому «B стартовала раньше конца A» здесь НЕ наблюдается — снятие
/// ожидания тест порядка не ломает. Этот тест доказывает не порядок, а то, что режим
/// без зависимости работает и итог A верен; сам порядок на одной очереди подтверждают данные
/// и метки главного теста (они были бы верны и без ожидания), а настоящее различие
/// проявится только на телефоне/при нескольких очередях (smoke `gpu-fence`).
#[test]
fn control_run_without_dependency_reports_its_mode() {
    let Some(context) = gpu_context_or_skip() else { return };
    let report = fence_order_check(
        &context,
        FenceOrderParams {
            work_b_dependency: WorkBDependency::NoDependency,
            ..FenceOrderParams::default()
        },
    )
    .unwrap();
    assert_eq!(report.work_b_dependency, WorkBDependency::NoDependency);
    assert_eq!(report.work_a_value, report.expected_value);
    eprintln!("контрольный прогон без зависимости: {report:#?}");
    assert_no_validation_errors(&context);
}
