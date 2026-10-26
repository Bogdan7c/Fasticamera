//! Сбой `detach` Vulkan-бэкенда (GPU завис): окно возвращается, а Vulkan-объекты — в том числе
//! пайплайн тестового паттерна, который могут исполнять кадры в полёте, — намеренно остаются
//! в утечке, а не уничтожаются (VUID-vkDestroyPipeline-00765). Зависший GPU изображает шов отказа.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod vulkan_support;

use std::sync::Arc;

use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::swapchain::InjectedFault;
use render_loop::backend::{DisplayBackend, FrameTiming, PresentOutcome};
use render_loop::config::RenderLoopConfig;
use shell_contract::clock::MonotonicNanos;
use support::{Journal, fake_surface};
use vulkan_support::{START_SIZE, headless_backend_or_skip};

fn timing(frame_index: u64) -> FrameTiming {
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        expected_present: MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр")),
        skipped_vsyncs: 0,
    }
}

#[test]
fn failed_detach_returns_the_window_counts_the_failure_and_keeps_the_pipeline_alive() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    backend.attach(fake_surface(&journal, 1), START_SIZE).expect("attach");
    assert_eq!(backend.render_and_present(timing(0)), PresentOutcome::Presented);
    let pipelines_while_attached = backend.context().live_objects().count(LiveObjectKind::Pipeline);
    assert!(pipelines_while_attached >= 1, "пайплайн паттерна создан");

    backend
        .inject_fault(InjectedFault::StuckGpu)
        .expect("шов зависшего GPU");
    let window = backend.detach();

    assert!(
        window.is_some(),
        "окно возвращается даже при зависшем GPU: оболочка не должна ждать"
    );
    assert_eq!(backend.diagnostics().detach_failures, 1);
    assert_eq!(
        backend.context().live_objects().count(LiveObjectKind::Pipeline),
        pipelines_while_attached,
        "пайплайн нельзя уничтожать, пока кадры в полёте могут его исполнять"
    );
}

#[test]
fn dropping_a_backend_whose_gpu_never_idles_also_keeps_the_pipeline_alive() {
    let Some(mut backend) = headless_backend_or_skip(&RenderLoopConfig::default()) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    backend.attach(fake_surface(&journal, 1), START_SIZE).expect("attach");
    assert_eq!(backend.render_and_present(timing(0)), PresentOutcome::Presented);
    let context = backend.context().clone();
    let pipelines_while_attached = context.live_objects().count(LiveObjectKind::Pipeline);

    backend
        .inject_fault(InjectedFault::StuckGpu)
        .expect("шов зависшего GPU");
    drop(backend);

    assert_eq!(
        context.live_objects().count(LiveObjectKind::Pipeline),
        pipelines_while_attached,
        "Drop показа действует по тому же правилу утечки, что и release"
    );
}
