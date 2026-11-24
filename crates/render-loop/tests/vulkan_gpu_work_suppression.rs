//! Подавление работы GPU для A/B-замера тока (16a) через НАСТОЯЩИЙ бэкенд показа:
//! * подавленные приборы не считаются (проходы не растут, объекты освобождены), запрос экрана помнится, а после
//!   снятия подавления приборы возобновляются САМИ — без новой команды `Scopes(Show)`;
//! * подавленный UI не записывается в GPU (`frames_drawn` стоит, растёт `frames_suppressed`), но источник по-прежнему
//!   получает свой кадр на каждом показанном кадре; приборы при этом продолжают считаться и ничем не «залипают»;
//! * подавление не меняет рисовальщик, стадии ISP, запрос приборов и установленный источник; повтор — `AlreadyInEffect`.
//!
//! Пиксели итогового кадра здесь не сверяются: цель показа headless-стенда прочитать нельзя. Равенство кадра «только
//! компоновщик» проверяет GPU-тест на читаемой цели (`pipelines/ui_pass/suppression_tests.rs`).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod scope_rig;
mod support;
mod viewfinder_rig;
mod vulkan_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use render_loop::backend::DisplayBackend;
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use render_loop::viewfinder::{
    GpuWorkSuppression, ScopeResources, SuppressibleGpuWork, ViewfinderCommand, ViewfinderCommandOutcome,
    ViewfinderDiagnostics,
};
use scope_contract::ScopeIntent;
use scope_rig::{
    FRAMES_TO_FLUSH_SLOTS, WARM_UP_FRAMES, apply_scopes, flush_frames, install, scopes_of, start, start_without_ui,
    waveform,
};
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;
use viewfinder_rig::{ViewfinderBench, neutral_gray};

/// Источник UI, который считает свои кадры: «раз на показанный кадр» видно по счётчику, а не по догадке.
struct CountingSource {
    frames: Arc<AtomicU64>,
}

impl UiFrameSource for CountingSource {
    fn name(&self) -> &'static str {
        "counting-suppression-test"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        self.frames.fetch_add(1, Ordering::SeqCst);
        Ok(DrawList::new(LightDirection::from_angle_degrees(45.0)))
    }
}

fn suppress(bench: &mut ViewfinderBench, suppression: GpuWorkSuppression) -> ViewfinderCommandOutcome {
    bench
        .backend
        .apply_viewfinder_command(ViewfinderCommand::SuppressGpuWork(suppression))
}

fn viewfinder_of(bench: &mut ViewfinderBench) -> ViewfinderDiagnostics {
    bench
        .backend
        .diagnostics()
        .viewfinder
        .expect("диагностика видоискателя")
}

const SCOPES: GpuWorkSuppression = GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::ColorScopes);
const UI: GpuWorkSuppression = GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::UiOverlayDrawing);

#[test]
fn suppressed_scopes_stop_counting_keep_the_screen_request_and_resume_by_themselves() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 40];
    let Some(mut bench) = start(&scenes) else { return };
    flush_frames(&mut bench, WARM_UP_FRAMES);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 3);
    let computing = scopes_of(&mut bench);
    assert_eq!(computing.residency.resources, ScopeResources::Live);
    assert!(computing.counters.passes_recorded >= 3, "{computing:?}");
    let product = viewfinder_of(&mut bench);
    assert_eq!(
        product.gpu_work_suppression,
        GpuWorkSuppression::NONE,
        "продукт ничего не подавляет"
    );

    // Подавили приборы: проходов больше нет, объекты освобождены, запрос экрана остался.
    assert_eq!(suppress(&mut bench, SCOPES), ViewfinderCommandOutcome::Applied);
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS + 2);
    let suppressed = scopes_of(&mut bench);
    assert_eq!(
        suppressed.counters.passes_recorded, computing.counters.passes_recorded,
        "подавленные приборы не записывают проходов"
    );
    assert_eq!(
        suppressed.residency.resources,
        ScopeResources::Absent,
        "объекты приборов освобождены"
    );
    assert_eq!(
        suppressed.counters.resources_created, suppressed.counters.resources_released,
        "ничего не залипло"
    );
    assert_eq!(
        suppressed.requested,
        Some(waveform()),
        "запрос экрана не стёрт подавлением"
    );
    assert_eq!(suppressed.withdrawals, 0, "подавление — не отзыв запроса");
    let during = viewfinder_of(&mut bench);
    assert_eq!(
        during.gpu_work_suppression, SCOPES,
        "диагностика показывает действующую маску"
    );
    assert_eq!(
        (during.painter, during.stages),
        (product.painter, product.stages),
        "рисовальщик и стадии ISP подавление не трогает"
    );
    assert_eq!(
        during.ui_overlay.active_source.as_deref(),
        Some("idle-test"),
        "источник UI остался установленным"
    );
    assert_eq!(
        suppress(&mut bench, SCOPES),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "повтор того же набора"
    );

    // Сняли подавление БЕЗ новой команды `Scopes(Show)`: приборы возобновились по запомненному запросу.
    assert_eq!(
        suppress(&mut bench, GpuWorkSuppression::NONE),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 3);
    let resumed = scopes_of(&mut bench);
    assert_eq!(resumed.residency.resources, ScopeResources::Live, "набор поднят заново");
    assert!(
        resumed.counters.passes_recorded > suppressed.counters.passes_recorded,
        "проходы возобновились: было {}, стало {}",
        suppressed.counters.passes_recorded,
        resumed.counters.passes_recorded
    );
    assert_eq!(resumed.requested, Some(waveform()));
    bench.finish();
}

#[test]
fn suppressed_ui_is_not_recorded_but_its_source_still_gets_every_shown_frame_and_scopes_keep_working() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 60];
    let Some(mut bench) = start_without_ui(&scenes) else {
        return;
    };
    let source_frames = Arc::new(AtomicU64::new(0));
    install(
        &mut bench,
        Box::new(CountingSource {
            frames: Arc::clone(&source_frames),
        }),
    );
    flush_frames(&mut bench, WARM_UP_FRAMES);
    assert_eq!(
        apply_scopes(&mut bench, ScopeIntent::Show(waveform())),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 3);
    let before_ui = viewfinder_of(&mut bench);
    let before_scopes = before_ui.scopes.clone();
    let calls_before = source_frames.load(Ordering::SeqCst);
    assert!(
        before_ui.ui_overlay.frames_drawn > 0,
        "UI рисуется: {:?}",
        before_ui.ui_overlay
    );
    assert_eq!(before_ui.ui_overlay.frames_suppressed, 0);

    // UI подавлен: несколько десятков кадров.
    assert_eq!(suppress(&mut bench, UI), ViewfinderCommandOutcome::Applied);
    flush_frames(&mut bench, 30);
    let during = viewfinder_of(&mut bench);
    let calls_during = source_frames.load(Ordering::SeqCst) - calls_before;
    assert!(
        calls_during >= 30,
        "источник вызывался на каждом показанном кадре: {calls_during}"
    );
    assert_eq!(
        during.ui_overlay.frames_drawn, before_ui.ui_overlay.frames_drawn,
        "в GPU ничего не записано"
    );
    assert_eq!(
        during.ui_overlay.frames_suppressed, calls_during,
        "каждый вызов источника на подавленном кадре посчитан"
    );
    assert_eq!(
        (during.ui_overlay.frames_failed, during.ui_overlay.source_failures),
        (0, 0),
        "подавление — не сбой"
    );
    assert_eq!(
        during.ui_overlay.active_source.as_deref(),
        Some("counting-suppression-test")
    );
    assert_eq!(during.gpu_work_suppression, UI);
    // Приборы продолжают считаться и ничем не залипли: проходы растут, картинка UI не берётся.
    assert_eq!(during.scopes.residency.resources, ScopeResources::Live);
    assert!(
        during.scopes.counters.passes_recorded >= before_scopes.counters.passes_recorded + 30,
        "приборы считались на каждом новом кадре: было {}, стало {}",
        before_scopes.counters.passes_recorded,
        during.scopes.counters.passes_recorded
    );
    assert_eq!(
        during.scopes.counters.picture_handoffs, before_scopes.counters.picture_handoffs,
        "картинка прибора UI не отдавалась"
    );
    assert_eq!(
        (during.painter, during.stages, during.scopes.requested),
        (before_ui.painter, before_ui.stages, before_ui.scopes.requested)
    );

    // Сняли подавление: UI снова на кадре, картинка прибора снова отдаётся.
    assert_eq!(
        suppress(&mut bench, GpuWorkSuppression::NONE),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 3);
    let resumed = viewfinder_of(&mut bench);
    assert!(
        resumed.ui_overlay.frames_drawn > during.ui_overlay.frames_drawn,
        "UI снова записывается"
    );
    assert_eq!(
        resumed.ui_overlay.frames_suppressed,
        during.ui_overlay.frames_suppressed
    );
    assert!(resumed.scopes.counters.picture_handoffs > during.scopes.counters.picture_handoffs);

    // Источник ушёл: наборы приборов освобождаются без утечек (нет вечно занятых слотов).
    assert_eq!(
        bench.backend.apply_ui_overlay_command(UiOverlayCommand::Remove),
        UiOverlayOutcome::Applied
    );
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS);
    let released = scopes_of(&mut bench);
    assert_eq!(released.residency.resources, ScopeResources::Absent);
    assert_eq!(
        released.counters.resources_created,
        released.counters.resources_released
    );
    bench.finish();
}

#[test]
fn both_works_suppressed_leave_only_the_compositor_and_lifting_restores_the_product() {
    let scenes = vec![neutral_gray(); WARM_UP_FRAMES + 30];
    let Some(mut bench) = start_without_ui(&scenes) else {
        return;
    };
    let source_frames = Arc::new(AtomicU64::new(0));
    install(
        &mut bench,
        Box::new(CountingSource {
            frames: Arc::clone(&source_frames),
        }),
    );
    flush_frames(&mut bench, WARM_UP_FRAMES);
    apply_scopes(&mut bench, ScopeIntent::Show(waveform()));
    flush_frames(&mut bench, 2);
    let before = viewfinder_of(&mut bench);

    let both = SCOPES.suppressing(SuppressibleGpuWork::UiOverlayDrawing);
    assert_eq!(suppress(&mut bench, both), ViewfinderCommandOutcome::Applied);
    flush_frames(&mut bench, FRAMES_TO_FLUSH_SLOTS + 4);
    let during = viewfinder_of(&mut bench);
    assert_eq!(
        during.scopes.counters.passes_recorded,
        before.scopes.counters.passes_recorded
    );
    assert_eq!(during.ui_overlay.frames_drawn, before.ui_overlay.frames_drawn);
    assert!(during.ui_overlay.frames_suppressed > 0);
    assert_eq!(during.scopes.residency.resources, ScopeResources::Absent);
    assert!(
        during.counters.frames_processed > before.counters.frames_processed,
        "видоискатель при этом идёт как обычно"
    );

    assert_eq!(
        suppress(&mut bench, GpuWorkSuppression::NONE),
        ViewfinderCommandOutcome::Applied
    );
    flush_frames(&mut bench, 3);
    let after = viewfinder_of(&mut bench);
    assert_eq!(after.gpu_work_suppression, GpuWorkSuppression::NONE);
    assert!(after.scopes.counters.passes_recorded > during.scopes.counters.passes_recorded);
    assert!(after.ui_overlay.frames_drawn > during.ui_overlay.frames_drawn);
    bench.finish();
}

#[test]
fn product_backend_is_already_without_suppression_so_none_is_a_repeat() {
    let scenes = vec![neutral_gray(); 4];
    let Some(mut bench) = start(&scenes) else { return };
    assert_eq!(
        suppress(&mut bench, GpuWorkSuppression::NONE),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "продукт уже без подавления"
    );
    assert_eq!(viewfinder_of(&mut bench).gpu_work_suppression, GpuWorkSuppression::NONE);
    bench.finish();
}
