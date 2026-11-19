//! Экран камеры поверх видоискателя через НАСТОЯЩИЙ бэкенд показа (сессия 12, решение 93): команда
//! «кадрирование + место» одной транзакцией и UI на чёрном кадре, когда камера не дала изображения.
//! Тот же стенд, что у `vulkan_ui_overlay.rs` (RADV, headless-показ, полная validation).
//!
//! Цель показа (swapchain) headless-стенда прочитать нельзя, поэтому ПИКСЕЛИ места видоискателя и UI на
//! чёрном проверяют тесты на читаемой offscreen-цели в самом crate (`pipelines/screen_tests.rs`). Здесь —
//! сквозной путь: команда → бэкенд → кадры, итоги команд, диагностика места, изображение ISP с новым
//! кадрированием, переживание пересоздания surface и то, что UI рисуется без единого кадра камеры.

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

use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop, ZoomFactor};
use render_loop::backend::DisplayBackend;
use render_loop::config::{RenderLoopConfig, UiOverlayConfig, ViewfinderPlacement};
use render_loop::viewfinder::{
    EffectivePlacement, FramingDestination, NormalizedScreenRect, ViewfinderCommand, ViewfinderCommandOutcome,
    ViewfinderFraming,
};
use ui_overlay_rig::*;

fn lower_half() -> NormalizedScreenRect {
    NormalizedScreenRect::new(0.0, 0.5, 1.0, 0.5).expect("прямоугольник")
}

fn square_framing(destination: FramingDestination) -> ViewfinderFraming {
    ViewfinderFraming {
        crop: ViewfinderCrop {
            aspect: CropAspect::SQUARE,
            zoom: ZoomFactor::NONE,
        },
        destination,
    }
}

#[test]
fn framing_command_reports_each_outcome_and_the_effective_place_follows_it() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let configured = RenderLoopConfig::default().viewfinder.placement;
    assert_eq!(
        harness.viewfinder().placement,
        EffectivePlacement::Configured(configured),
        "до команды место — по конфигу"
    );
    let landscape = harness.readback();

    let command = ViewfinderCommand::SetFraming(square_framing(FramingDestination::ScreenRect(lower_half())));
    assert_eq!(
        harness.backend.apply_viewfinder_command(command),
        ViewfinderCommandOutcome::Applied
    );
    // Кадрирование и место поменялись ОДНОЙ командой: в диагностике новое место, а следующий кадр ISP
    // уже квадратный — «нового кадрирования в старом месте» не бывает.
    assert_eq!(
        harness.viewfinder().placement,
        EffectivePlacement::ScreenRect(lower_half())
    );
    harness.show_new_camera_frame();
    let square = harness.readback();
    assert_eq!(
        square.width, square.height,
        "квадратное кадрирование дошло до ISP: {}x{}",
        square.width, square.height
    );
    assert_ne!(
        (landscape.width, landscape.height),
        (square.width, square.height),
        "изображение действительно сменило формат"
    );

    assert_eq!(
        harness.backend.apply_viewfinder_command(command),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "повтор ничего не меняет"
    );

    // Сломанное кадрирование отвергается ЦЕЛИКОМ: место тоже остаётся прежним.
    let broken = ViewfinderCommand::SetFraming(ViewfinderFraming {
        crop: ViewfinderCrop {
            aspect: CropAspect::LongToShort { long: 1, short: 2 },
            zoom: ZoomFactor::NONE,
        },
        destination: FramingDestination::Configured,
    });
    let outcome = harness.backend.apply_viewfinder_command(broken);
    assert!(
        matches!(outcome, ViewfinderCommandOutcome::Rejected { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        harness.viewfinder().placement,
        EffectivePlacement::ScreenRect(lower_half()),
        "отказ не тронул место"
    );

    // `SetCrop` меняет только кадрирование и место не трогает.
    let wide = ViewfinderCrop {
        aspect: CropAspect::LongToShort { long: 16, short: 9 },
        zoom: ZoomFactor::NONE,
    };
    assert_eq!(
        harness
            .backend
            .apply_viewfinder_command(ViewfinderCommand::SetCrop(wide)),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(
        harness.viewfinder().placement,
        EffectivePlacement::ScreenRect(lower_half())
    );

    // Возврат к настроенному месту.
    let back = ViewfinderCommand::SetFraming(ViewfinderFraming {
        crop: wide,
        destination: FramingDestination::Configured,
    });
    assert_eq!(
        harness.backend.apply_viewfinder_command(back),
        ViewfinderCommandOutcome::Applied
    );
    assert_eq!(
        harness.viewfinder().placement,
        EffectivePlacement::Configured(ViewfinderPlacement::default())
    );
    harness.finish();
}

#[test]
fn the_rect_is_a_fraction_of_the_target_and_survives_surface_recreation_with_another_size() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    // Полное кадрирование: у крошечного тестового кадра после квадратного кропа не хватает ячеек для сетки
    // статистики AWB на первом кадре новой сессии (свойство тестовых данных, не рендера).
    let command = ViewfinderCommand::SetFraming(ViewfinderFraming {
        crop: ViewfinderCrop::FULL_SENSOR,
        destination: FramingDestination::ScreenRect(lower_half()),
    });
    assert_eq!(
        harness.backend.apply_viewfinder_command(command),
        ViewfinderCommandOutcome::Applied
    );
    harness.show_new_camera_frame();
    // Пересоздание surface другого размера: доли не зависят от пикселей, команду повторять не нужно.
    harness.detach();
    harness.attach(2, RESIZED_SURFACE);
    harness.show_new_camera_frame();
    assert_eq!(
        harness.viewfinder().placement,
        EffectivePlacement::ScreenRect(lower_half()),
        "место пережило пересоздание surface"
    );
    assert_eq!(
        harness.backend.apply_viewfinder_command(command),
        ViewfinderCommandOutcome::AlreadyInEffect,
        "бэкенд помнит применённое"
    );
    harness.finish();
}

#[test]
fn ui_is_drawn_on_every_frame_even_when_the_camera_has_given_no_image_at_all() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    let probe = SourceProbe::new();
    harness.install(&probe);
    // Ни одного кадра камеры: экран чёрный, но UI на нём есть (раньше на чёрном кадре UI не рисовался).
    for _ in 0..4 {
        harness.present();
    }
    let ui = harness.ui();
    assert_eq!(ui.frames_drawn, 4, "{ui:?}");
    assert_eq!(probe.calls(), 4);
    assert_eq!(
        harness.viewfinder().counters.frames_processed,
        0,
        "камера кадров не давала"
    );
    assert_eq!((ui.source_failures, ui.frames_failed), (0, 0), "{ui:?}");

    // Камера дала кадр, потом поток закончился: снова чёрный кадр, UI продолжает рисоваться.
    harness.show_new_camera_frame();
    let drawn_with_camera = harness.ui().frames_drawn;
    harness.stream.producer().end_stream(harness.epoch);
    for _ in 0..3 {
        harness.present();
    }
    assert_eq!(
        harness.ui().frames_drawn,
        drawn_with_camera + 3,
        "после остановки потока UI рисуется поверх чёрного"
    );
    harness.detach();
    assert_eq!(harness.stream.counters().leaked_leases, 0);
}

#[test]
fn a_failing_source_on_the_black_frame_is_counted_and_nothing_else_breaks() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    let probe = SourceProbe::new();
    probe.failing.store(true, std::sync::atomic::Ordering::SeqCst);
    harness.install(&probe);
    for _ in 0..3 {
        harness.present();
    }
    let failed = harness.ui();
    assert_eq!((failed.source_failures, failed.frames_drawn), (3, 0), "{failed:?}");
    // Источник пришёл в себя — рисование возобновилось, кадры камеры идут как обычно.
    probe.failing.store(false, std::sync::atomic::Ordering::SeqCst);
    harness.show_new_camera_frame();
    assert!(harness.ui().frames_drawn >= 1);
    assert!(harness.viewfinder().counters.frames_processed >= 1);
    harness.finish();
}

#[test]
fn without_a_source_the_black_frame_draws_no_ui() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    for _ in 0..3 {
        harness.present();
    }
    assert_eq!(harness.ui().frames_drawn, 0);
    harness.finish();
}

#[test]
fn without_a_camera_the_framing_command_is_unsupported() {
    let render_config = RenderLoopConfig::default();
    let Some(mut backend) = vulkan_support::headless_backend_or_skip(&render_config) else {
        return;
    };
    assert_eq!(
        backend.apply_viewfinder_command(ViewfinderCommand::SetFraming(square_framing(
            FramingDestination::ScreenRect(lower_half())
        ))),
        ViewfinderCommandOutcome::Unsupported
    );
}
