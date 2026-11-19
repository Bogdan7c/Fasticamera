//! UI и стекло поверх живого видоискателя через НАСТОЯЩИЙ бэкенд показа (сессия 11): `VulkanDisplayBackend`
//! в режиме камеры на реальном драйвере (RADV, headless-показ, полная validation) + настоящий
//! `RawFrameStream` и тестовый импортёр синтетического RAW. Источник кадра UI — тестовый, со стеклом.
//!
//! Доказывается:
//! * по умолчанию UI выключен: без источника рендер UI не создаётся (число объектов GPU не растёт), счётчики
//!   пусты; появляется только после установки источника;
//! * с источником каждый показанный кадр видоискателя получает кадр UI поверх: `frames_drawn` растёт на
//!   каждый кадр, пирамида размытия перестраивается на новый кадр камеры и переиспользуется на перекомпоновке,
//!   время GPU замерено, validation чиста; источник получает логический размер экрана, `px_per_pt` по
//!   правилу ширины 440 pt и ожидаемое время показа именно этого кадра;
//! * UI не меняет изображение видоискателя: пиксели `read_viewfinder_image` и счётчики видоискателя те же,
//!   что в прогоне без UI на тех же кадрах;
//! * отказ источника и превышение предела кадра не роняют показ камеры: кадры камеры идут, отказы считаются;
//! * `Remove` останавливает рисование (`removals == 1`); источник переживает пересоздание surface и смену
//!   размера; в конце все объекты Vulkan вернулись к исходным;
//! * весь путь «команда в почтовом ящике → поток рендера → бэкенд» работает на настоящем потоке.
//!
//! Цель показа (swapchain) на headless-стенде прочитать нельзя, поэтому пиксели самого UI здесь не
//! сверяются (это делают тесты `ui-render-vulkan` на цели, которую можно читать); тест доказывает путь
//! кадра: источник вызван, рендер записал кадр, validation молчит, видоискатель не пострадал.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod camera_rig;
mod support;
mod ui_overlay_rig;
mod vulkan_support;

use std::sync::atomic::Ordering;

use camera_rig::{Camera, start_rig};
use render_loop::backend::DisplayBackend;
use render_loop::config::{RenderLoopConfig, UiOverlayConfig};
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayDiagnostics, UiOverlayOutcome};
use render_loop::vulkan_backend::ViewfinderReadback;
use support::wait_until;
use ui_draw::geometry::PxSize;
use ui_overlay_rig::*;
use ui_render_vulkan::UiRenderLimits;
use vulkan_support::assert_no_validation_errors;

// ---- тесты ----

#[test]
fn ui_is_off_by_default_and_the_renderer_appears_only_with_a_source() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    for _ in 0..3 {
        harness.show_new_camera_frame();
        harness.recompose(2);
    }
    assert_eq!(
        harness.ui(),
        UiOverlayDiagnostics::default(),
        "без источника в диагностике UI пусто: рендер не работал"
    );
    let objects_without_ui = harness.live_object_total();

    let probe = SourceProbe::new();
    harness.install(&probe);
    // Рендер UI создаётся СРАЗУ при установке, между кадрами (ревью 11): первый кадр с UI не платит
    // за конвейеры и загрузку атласа своим vsync.
    assert!(
        harness.live_object_total() > objects_without_ui,
        "рендер UI создан при установке, ещё до кадра: было {objects_without_ui}, стало {}",
        harness.live_object_total()
    );
    harness.show_new_camera_frame();
    let with_source = harness.ui();
    assert_eq!(with_source.active_source.as_deref(), Some(SOURCE_NAME));
    assert_eq!(with_source.installs, 1);
    assert!(with_source.frames_drawn >= 1, "{with_source:?}");
    assert!(
        harness.live_object_total() > objects_without_ui,
        "рендер UI создан только после установки источника: было {objects_without_ui}, стало {}",
        harness.live_object_total()
    );

    // Снятие: источник уничтожен на стороне бэкенда, дальше он не вызывается и кадры UI не растут.
    assert_eq!(harness.remove(), UiOverlayOutcome::Applied);
    assert!(probe.dropped.load(Ordering::SeqCst), "снятый источник уничтожен");
    let calls_after_remove = probe.calls();
    let drawn_after_remove = harness.ui().frames_drawn;
    harness.show_new_camera_frame();
    harness.recompose(3);
    let removed = harness.ui();
    assert_eq!(probe.calls(), calls_after_remove, "после Remove источник не вызывается");
    assert_eq!(removed.frames_drawn, drawn_after_remove, "после Remove UI не рисуется");
    assert_eq!((removed.installs, removed.removals), (1, 1));
    assert_eq!(removed.active_source, None);
    assert_eq!(
        harness.remove(),
        UiOverlayOutcome::AlreadyInEffect,
        "повторное снятие ничего не меняет"
    );
    assert_eq!(harness.ui().removals, 1);
    harness.finish();
}

#[test]
fn glass_is_drawn_on_every_shown_frame_with_a_rebuilt_then_reused_pyramid() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let probe = SourceProbe::new();
    harness.install(&probe);

    // 4 новых кадра камеры, после каждого по 2 перекомпоновки.
    let mut presents_with_ui = 0;
    for _ in 0..4 {
        presents_with_ui += harness.show_new_camera_frame();
        harness.recompose(2);
        presents_with_ui += 2;
    }
    // Ещё кадры, чтобы метки времени всех слотов были собраны после их fence.
    harness.recompose(4);
    presents_with_ui += 4;

    let ui = harness.ui();
    assert_eq!(
        ui.frames_drawn, presents_with_ui,
        "кадр UI на каждый показанный кадр: {ui:?}"
    );
    assert_eq!(probe.calls(), presents_with_ui, "источник вызван ровно раз на кадр");
    assert_eq!(
        ui.pyramid_rebuilds, 4,
        "пирамида перестроена по разу на новый кадр камеры: {ui:?}"
    );
    assert_eq!(
        ui.pyramid_reuses,
        presents_with_ui - 4,
        "на перекомпоновках пирамида переиспользована: {ui:?}"
    );
    assert_eq!(
        (ui.frames_failed, ui.source_failures, ui.limit_overflows),
        (0, 0, 0),
        "{ui:?}"
    );
    assert_eq!(ui.last_error, None);
    let gpu_time = ui.gpu_time.expect("время UI на GPU замерено");
    assert!(
        gpu_time.samples > 0 && gpu_time.max > std::time::Duration::ZERO,
        "{gpu_time:?}"
    );

    // Источник получил то, что обещает контракт: размер экрана, масштаб по ширине, время показа кадра.
    let requests = probe.requests();
    let last = requests.last().expect("запросы были");
    assert_eq!(
        last.logical_size,
        PxSize::new(SURFACE.width_px as f32, SURFACE.height_px as f32)
    );
    let expected_scale = SURFACE.width_px as f32 / ui_core::input::PointScaleRule::default().reference_width_pt as f32;
    assert!(
        (last.px_per_pt - expected_scale).abs() < 1e-4,
        "px_per_pt {} против {expected_scale}",
        last.px_per_pt
    );
    let present_times: Vec<i64> = requests.iter().map(|request| request.present_time.as_nanos()).collect();
    assert!(
        present_times.windows(2).all(|pair| pair[1] > pair[0]),
        "время показа растёт от кадра к кадру: {present_times:?}"
    );
    assert_eq!(
        *present_times.last().expect("запросы были"),
        PRESENT_STEP_NANOS * i64::try_from(harness.frame_index - 1).expect("кадр"),
        "последний запрос — с ожидаемым временем показа последнего кадра"
    );
    harness.finish();
}

#[test]
fn ui_leaves_the_viewfinder_image_and_counters_exactly_as_without_it() {
    let run = |with_ui: bool| -> Option<(ViewfinderReadback, [u64; 4])> {
        let mut harness = Harness::start(UiOverlayConfig::default())?;
        harness.attach(1, SURFACE);
        harness.show_new_camera_frame();
        let probe = SourceProbe::new();
        if with_ui {
            harness.install(&probe);
        }
        for _ in 0..3 {
            harness.show_new_camera_frame();
            harness.recompose(2);
        }
        let image = harness.readback();
        let viewfinder = harness.viewfinder();
        if with_ui {
            assert!(
                viewfinder.ui_overlay.frames_drawn >= 9,
                "UI действительно рисовался: {:?}",
                viewfinder.ui_overlay
            );
        } else {
            assert_eq!(probe.calls(), 0);
        }
        let result = (
            image,
            [
                viewfinder.counters.frames_processed,
                viewfinder.counters.recomposed,
                viewfinder.counters.compose_failures,
                viewfinder.counters.isp_record_failures,
            ],
        );
        harness.finish();
        Some(result)
    };
    let Some(without_ui) = run(false) else { return };
    let Some(with_ui) = run(true) else { return };
    assert_eq!(with_ui.0.width, without_ui.0.width);
    assert_eq!(with_ui.0.height, without_ui.0.height);
    assert_eq!(
        with_ui.0.pixels, without_ui.0.pixels,
        "изображение видоискателя с UI совпадает пиксель в пиксель с прогоном без UI"
    );
    assert_eq!(with_ui.1, without_ui.1, "счётчики видоискателя те же");
}

#[test]
fn a_failing_source_never_stops_the_camera_and_recovers_when_it_answers_again() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let probe = SourceProbe::new();
    probe.failing.store(true, Ordering::SeqCst);
    harness.install(&probe);

    let processed_before = harness.viewfinder().counters.frames_processed;
    let presented_before = harness
        .backend
        .diagnostics()
        .camera
        .expect("камера")
        .camera_frames_presented;
    for _ in 0..3 {
        harness.show_new_camera_frame();
        harness.recompose(1);
    }
    let failed = harness.ui();
    assert!(failed.source_failures >= 6, "каждый кадр — отказ источника: {failed:?}");
    assert_eq!(failed.frames_drawn, 0, "{failed:?}");
    assert_eq!(
        failed.frames_failed, 0,
        "отказ источника — не отказ рендера: {failed:?}"
    );
    assert!(
        failed
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("тестовый отказ")),
        "{failed:?}"
    );
    assert_eq!(failed.pyramid_rebuilds, 0, "без списка пирамида не нужна");
    let viewfinder = harness.viewfinder();
    assert_eq!(
        viewfinder.counters.frames_processed,
        processed_before + 3,
        "камера продолжает идти"
    );
    assert_eq!(viewfinder.counters.compose_failures, 0);
    let presented_after = harness
        .backend
        .diagnostics()
        .camera
        .expect("камера")
        .camera_frames_presented;
    assert_eq!(presented_after, presented_before + 3, "кадры камеры дошли до показа");

    // Источник снова отвечает: кадры UI идут, прежние отказы остаются в счётчике.
    probe.failing.store(false, Ordering::SeqCst);
    harness.show_new_camera_frame();
    harness.recompose(2);
    let recovered = harness.ui();
    assert!(recovered.frames_drawn >= 3, "{recovered:?}");
    assert_eq!(recovered.source_failures, failed.source_failures, "новых отказов нет");
    assert_eq!(recovered.frames_failed, 0);
    harness.finish();
}

#[test]
fn a_frame_over_the_limits_is_rejected_with_a_counter_and_the_viewfinder_stays_on_screen() {
    let config = UiOverlayConfig {
        limits: UiRenderLimits {
            max_glass_groups: 1,
            ..UiRenderLimits::default()
        },
        ..UiOverlayConfig::default()
    };
    let Some(mut harness) = Harness::start(config) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let probe = SourceProbe::new();
    probe.groups.store(2, Ordering::SeqCst);
    harness.install(&probe);
    harness.show_new_camera_frame();
    harness.recompose(2);
    let overflowed = harness.ui();
    assert_eq!(overflowed.frames_drawn, 0, "{overflowed:?}");
    assert_eq!(overflowed.frames_failed, 3, "{overflowed:?}");
    assert_eq!(overflowed.limit_overflows, 3, "{overflowed:?}");
    assert!(
        overflowed
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("групп стекла")),
        "{overflowed:?}"
    );
    assert_eq!(
        harness.viewfinder().counters.compose_failures,
        0,
        "видоискатель на экране"
    );

    // Источник стал укладываться в предел: рисование возобновилось на том же рендере.
    probe.groups.store(1, Ordering::SeqCst);
    harness.show_new_camera_frame();
    let fitting = harness.ui();
    assert!(fitting.frames_drawn >= 1, "{fitting:?}");
    assert_eq!(fitting.frames_failed, 3, "новых отказов нет");
    harness.finish();
}

#[test]
fn the_source_survives_surface_recreation_and_resizing() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let probe = SourceProbe::new();
    harness.install(&probe);
    harness.show_new_camera_frame();
    harness.recompose(2);
    let before_resize = harness.ui().frames_drawn;
    assert!(before_resize >= 3);

    // Смена размера цепочки под работающим UI: кадры идут, источник получает новый размер.
    harness.backend.resize(RESIZED_SURFACE).expect("пересоздание цепочки");
    harness.show_new_camera_frame();
    harness.recompose(2);
    let after_resize = harness.ui();
    assert!(after_resize.frames_drawn >= before_resize + 3, "{after_resize:?}");
    assert_eq!(
        (after_resize.frames_failed, after_resize.source_failures),
        (0, 0),
        "{after_resize:?}"
    );
    let last_size = probe.requests().last().expect("запросы были").logical_size;
    assert_eq!(
        last_size,
        PxSize::new(RESIZED_SURFACE.width_px as f32, RESIZED_SURFACE.height_px as f32)
    );

    // Отсоединение и новая surface: рендер UI ушёл с цепочкой, источник остался и рисует снова.
    harness.detach();
    let drawn_before_detach = harness.ui().frames_drawn;
    assert_eq!(
        harness.ui().active_source.as_deref(),
        Some(SOURCE_NAME),
        "источник пережил отсоединение"
    );
    harness.attach(2, SURFACE);
    harness.show_new_camera_frame();
    harness.recompose(2);
    let reattached = harness.ui();
    assert!(reattached.frames_drawn >= drawn_before_detach + 3, "{reattached:?}");
    assert_eq!(
        (reattached.frames_failed, reattached.source_failures),
        (0, 0),
        "{reattached:?}"
    );
    assert_eq!(reattached.installs, 1, "источник не переустанавливался");
    harness.finish();
}

#[test]
fn without_a_camera_the_command_is_unsupported_and_the_source_is_destroyed() {
    let render_config = RenderLoopConfig::default();
    let Some(mut backend) = vulkan_support::headless_backend_or_skip(&render_config) else {
        return;
    };
    let probe = SourceProbe::new();
    assert_eq!(
        backend.apply_ui_overlay_command(install_command(&probe)),
        UiOverlayOutcome::Unsupported
    );
    assert!(
        probe.dropped.load(Ordering::SeqCst),
        "источник не оседает в бэкенде без видоискателя"
    );
    assert_eq!(probe.calls(), 0);
    assert_eq!(
        backend.diagnostics().viewfinder,
        None,
        "диагностики видоискателя нет вовсе"
    );
}

#[test]
fn commands_reach_the_backend_through_the_render_thread_mailbox() {
    let Some(rig) = start_rig() else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);
    rig.show_surface(1);
    assert!(wait_until(camera_rig::DEADLINE, || rig
        .viewfinder()
        .counters
        .frames_processed
        >= 5));
    assert_eq!(
        rig.viewfinder().ui_overlay,
        UiOverlayDiagnostics::default(),
        "по умолчанию UI выключен"
    );

    let probe = SourceProbe::new();
    assert_eq!(
        rig.handle.inbox().ui_overlay_command(install_command(&probe)),
        render_loop::mailbox::InboxDelivery::Delivered
    );
    assert!(
        wait_until(camera_rig::DEADLINE, || rig.viewfinder().ui_overlay.frames_drawn >= 20),
        "UI не рисуется после установки: {:?}",
        rig.viewfinder().ui_overlay
    );
    let drawing = rig.viewfinder().ui_overlay;
    assert_eq!(drawing.active_source.as_deref(), Some(SOURCE_NAME));
    assert!(
        drawing.pyramid_rebuilds >= 1 && drawing.pyramid_reuses >= 1,
        "{drawing:?}"
    );
    assert!(wait_until(camera_rig::DEADLINE, || rig
        .viewfinder()
        .ui_overlay
        .gpu_time
        .is_some()));

    assert_eq!(
        rig.handle.inbox().ui_overlay_command(UiOverlayCommand::Remove),
        render_loop::mailbox::InboxDelivery::Delivered
    );
    assert!(wait_until(camera_rig::DEADLINE, || rig
        .viewfinder()
        .ui_overlay
        .removals
        == 1));
    let removed = rig.viewfinder().ui_overlay;
    assert_eq!(removed.active_source, None);
    let calls = probe.calls();
    let presented = rig.camera_presented();
    assert!(wait_until(camera_rig::DEADLINE, || rig.camera_presented() >= presented + 10));
    assert_eq!(probe.calls(), calls, "после Remove источник молчит, камера идёт дальше");
    assert!(probe.dropped.load(Ordering::SeqCst));

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(wait_until(camera_rig::DEADLINE, || rig.book.returned_count() == offered_total));
    rig.destroy_surface(1);
    assert!(wait_until(camera_rig::DEADLINE, || rig
        .handle
        .status()
        .display
        .is_none()));
    assert_eq!(rig.stream.counters().leaked_leases, 0);
    assert_eq!(rig.context.live_objects(), baseline);
    assert_no_validation_errors(&rig.context);
}
