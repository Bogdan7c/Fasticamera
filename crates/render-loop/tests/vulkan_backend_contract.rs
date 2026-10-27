//! Контракт `VulkanDisplayBackend` без потока: отсутствие surface, отказ attach, повторный
//! attach, кадры с немедленным detach (кадры в полёте!) и диагностика. Реальный драйвер,
//! полная validation.

// Стенд headless-показа существует только на ПК.
#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod vulkan_support;

use std::sync::Arc;

use render_loop::backend::{AttachError, DisplayBackend, FrameTiming, PresentOutcome, PresentTarget};
use render_loop::config::RenderLoopConfig;
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use support::{Journal, fake_surface};
use vulkan_support::{RESIZED, START_SIZE, assert_no_validation_errors, headless_backend_or_skip};

fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

#[test]
fn without_surface_every_call_is_a_clean_no_op_or_error() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let baseline = backend.context().live_objects();

    assert!(backend.detach().is_none(), "detach без surface — None");
    assert!(backend.resize(START_SIZE).is_err());
    assert!(matches!(
        backend.render_and_present(timing(0)),
        PresentOutcome::Failed(_)
    ));
    let diagnostics = backend.diagnostics();
    assert_eq!(diagnostics.display, None);
    assert!(
        backend.drain_past_presentations().is_empty(),
        "без цепочки записей о показах нет"
    );
    assert_eq!(diagnostics.detach_failures, 0);
    assert_eq!(backend.context().live_objects(), baseline, "ничего не создано");
    assert_no_validation_errors(backend.context());
}

#[test]
fn failed_attach_holds_nothing_and_releases_the_window() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    let baseline = backend.context().live_objects();

    // Нулевой размер: swapchain создать нельзя.
    let result = backend.attach(
        fake_surface(&journal, 1),
        SurfaceSize {
            width_px: 0,
            height_px: 0,
        },
    );

    assert!(matches!(result, Err(AttachError::SwapchainFailed(_))), "{result:?}");
    assert_eq!(journal.count("window_released(1)"), 1, "окно уничтожено при отказе");
    assert!(backend.detach().is_none(), "бэкенд ничего не держит");
    assert_eq!(backend.context().live_objects(), baseline);
    assert_eq!(backend.diagnostics().display, None);
    assert_no_validation_errors(backend.context());
}

#[test]
fn frames_resize_and_immediate_detach_return_the_same_window_without_validation_errors() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    let baseline = backend.context().live_objects();

    backend.attach(fake_surface(&journal, 7), START_SIZE).expect("attach");
    // Второй attach при живом первом отвергается, а чужое состояние не трогается.
    let second = backend.attach(fake_surface(&journal, 8), START_SIZE);
    assert!(matches!(second, Err(AttachError::SurfaceRejected(_))), "{second:?}");
    assert_eq!(journal.count("window_released(8)"), 1, "отвергнутое окно уничтожено");
    assert_eq!(journal.count("window_released(7)"), 0, "прежнее окно не тронуто");

    for frame in 0..10 {
        assert_eq!(backend.render_and_present(timing(frame)), PresentOutcome::Presented);
    }
    backend.resize(RESIZED).expect("resize");
    for frame in 10..20 {
        assert_eq!(backend.render_and_present(timing(frame)), PresentOutcome::Presented);
    }
    let display = backend.diagnostics().display.expect("цепочка есть");
    assert_eq!(display.extent, RESIZED);
    assert_eq!(display.recreation_count, 1);

    // Detach сразу после кадров: в полёте ещё работа GPU. Пайплайн паттерна обязан дожить
    // до конца этой работы (VUID-vkDestroyPipeline-00765 иначе даст ошибку validation).
    let returned = backend.detach().expect("окно возвращено");
    assert_eq!(returned.generation().value(), 7);
    assert_eq!(journal.count("window_released(7)"), 0, "окно ещё у вызывающего");
    drop(returned);
    assert_eq!(journal.count("window_released(7)"), 1);
    assert!(backend.detach().is_none(), "повторный detach — None");
    assert_eq!(backend.context().live_objects(), baseline);
    assert_eq!(backend.diagnostics().detach_failures, 0);
    assert_no_validation_errors(backend.context());
}

#[test]
fn swapchain_recreation_forgets_present_records_of_the_old_chain() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    assert_eq!(backend.present_history_len(), None, "без surface таблицы нет");
    let journal = Arc::new(Journal::default());
    backend.attach(fake_surface(&journal, 1), START_SIZE).expect("attach");
    for frame in 0..5 {
        assert_eq!(backend.render_and_present(timing(frame)), PresentOutcome::Presented);
    }
    assert_eq!(
        backend.present_history_len(),
        Some(5),
        "пять показов ждут записей display timing"
    );

    backend.resize(RESIZED).expect("resize");
    assert_eq!(backend.present_history_len(), Some(0), "записи старой цепочки забыты");

    assert_eq!(backend.render_and_present(timing(5)), PresentOutcome::Presented);
    assert_eq!(
        backend.present_history_len(),
        Some(1),
        "новая цепочка копит свои показы"
    );
    drop(backend.detach());
    assert_no_validation_errors(backend.context());
}

#[test]
fn dropping_backend_with_attached_display_leaves_no_objects_and_no_validation_errors() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    let context = backend.context().clone();
    let baseline = context.live_objects();
    backend.attach(fake_surface(&journal, 1), START_SIZE).expect("attach");
    for frame in 0..5 {
        assert_eq!(backend.render_and_present(timing(frame)), PresentOutcome::Presented);
    }
    drop(backend);
    assert_eq!(journal.count("window_released(1)"), 1);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn diagnostics_report_live_objects_by_kind_and_return_to_the_idle_census_after_detach() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    let idle = backend.diagnostics();

    backend.attach(fake_surface(&journal, 9), START_SIZE).expect("attach");
    let attached = backend.diagnostics();
    assert_eq!(
        attached.live_objects_by_kind.values().sum::<u64>(),
        attached.live_objects,
        "разрез по видам даёт ту же сумму, что и общий счётчик"
    );
    assert!(
        attached.live_objects_by_kind.get("Swapchain").copied().unwrap_or(0) >= 1,
        "цепочка показа видна в переписи: {:?}",
        attached.live_objects_by_kind
    );
    assert_ne!(attached.live_objects_by_kind, idle.live_objects_by_kind);

    drop(backend.detach().expect("окно возвращено"));
    assert_eq!(
        backend.diagnostics().live_objects_by_kind,
        idle.live_objects_by_kind,
        "в покое перепись по видам такая же, как до attach"
    );
    assert_no_validation_errors(backend.context());
}

/// Сессия 07: настоящий бэкенд меряет CPU кадра (время в acquire и путь захват → сабмит) и
/// выдаёт замер ровно один раз; без цепочки замера нет.
#[test]
fn real_backend_reports_frame_cpu_timing_once_per_frame() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    assert!(matches!(
        backend.render_and_present(timing(0)),
        PresentOutcome::Failed(_)
    ));
    assert_eq!(backend.take_frame_cpu_timing(), None, "без цепочки кадра не было");

    backend.attach(fake_surface(&journal, 3), START_SIZE).expect("attach");
    assert_eq!(backend.render_and_present(timing(1)), PresentOutcome::Presented);
    let cpu = backend.take_frame_cpu_timing().expect("кадр показан — замер есть");
    let latch_to_submit = cpu.latch_to_submit.expect("кадр дошёл до показа");
    assert!(
        latch_to_submit > std::time::Duration::ZERO,
        "запись и сабмит не бесплатны"
    );
    assert!(
        latch_to_submit < std::time::Duration::from_secs(1),
        "замер правдоподобен: {latch_to_submit:?}"
    );
    assert_eq!(backend.take_frame_cpu_timing(), None, "замер выдаётся один раз");

    drop(backend.detach());
    assert_no_validation_errors(backend.context());
}
