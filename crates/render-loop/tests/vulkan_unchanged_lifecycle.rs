//! Жизненный цикл показа под пропуском тиков без изменений (решение 214, сессия 16a) на НАСТОЯЩЕМ потоке рендера,
//! бэкенде камеры на реальном драйвере (headless, полная validation) и потоке-камере 60 к/с при экране 120 Гц:
//! * HOME и возврат (`Paused` → `Resumed`): придержанная картинка переживает паузу, а «что на экране» после неё
//!   неизвестно — первый тик после возврата рисуется (`drawn_no_baseline`; камера на это время молчит, чтобы причина
//!   отрисовки была однозначной), потом пропуски продолжаются;
//! * уничтожение surface с придержанной картинкой: квитанция подтверждается в бюджете, GPU-объекты возвращаются,
//!   validation чиста; новая surface начинает с нарисованного кадра;
//! * пока тики пропускаются, на руках у показа не больше кадров камеры, чем без пропуска, кадры возвращаются
//!   безопасно, а статистика AWB публикуется не реже, чем при показе каждого тика.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod camera_rig;
mod support;
mod vulkan_support;

use std::time::Duration;

use camera_rig::*;
use render_loop::config::UnchangedFramePolicy;
use render_loop::thread::RenderState;
use support::wait_until;

const SKIP: UnchangedFramePolicy = UnchangedFramePolicy::SkipWhenUnchanged;

fn skipped(rig: &Rig) -> u64 {
    rig.handle.status().ticks_skipped_unchanged
}

#[test]
fn home_and_return_with_a_held_image_forgets_the_shown_frame_and_keeps_skipping_afterwards() {
    let Some(rig) = start_rig_with_policy(SKIP) else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, _offered) = Camera::start(&rig.stream, &rig.log, &rig.book);
    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || skipped(&rig) >= 10),
        "пропуски не пошли: {:?}",
        rig.handle.status()
    );

    // HOME: показ на паузе, камера продолжает отдавать кадры, тики не идут.
    rig.handle.inbox().paused();
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Paused));
    let skipped_at_pause = skipped(&rig);
    let no_baseline_before = rig.viewfinder().counters.unchanged_frames.drawn_no_baseline;

    // Камера на время возврата молчит: иначе на первом тике после паузы в половине случаев оказывается новый кадр
    // камеры (экран 120 Гц, камера 60 к/с), тик рисуется по причине «новый кадр», эталон восстанавливается им же, и
    // счётчик `drawn_no_baseline` не растёт — тест падал примерно раз из пяти. Без новых кадров первый тик после
    // возврата обязан нарисоваться именно потому, что «что на экране» неизвестно.
    camera.stop_offering();

    // Возврат: первый тик рисуется без эталона, дальше пропуски идут снова (UI не меняется, кадров камеры нет).
    rig.handle.inbox().resumed();
    assert!(wait_until(DEADLINE, || rig.handle.status().state == RenderState::Presenting));
    assert!(
        wait_until(DEADLINE, || rig
            .viewfinder()
            .counters
            .unchanged_frames
            .drawn_no_baseline
            > no_baseline_before),
        "после возврата эталон не забыт: {:?}",
        rig.viewfinder().counters.unchanged_frames
    );
    assert!(
        wait_until(DEADLINE, || skipped(&rig) > skipped_at_pause + 10),
        "после возврата пропуски не продолжились: {:?}",
        rig.handle.status()
    );
    assert_eq!(rig.handle.status().backend_errors, 0);

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    rig.destroy_surface(1);
    let status = rig.handle.status();
    assert_eq!(status.backend.validation_errors, 0, "{status:?}");
    assert_eq!(
        rig.context.live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    let counters = rig.stream.counters();
    assert_eq!(
        (counters.leaked_leases, counters.frames_outstanding),
        (0, 0),
        "{counters:?}"
    );
}

#[test]
fn destroying_and_recreating_the_surface_under_skipping_confirms_and_leaks_nothing() {
    let Some(rig) = start_rig_with_policy(SKIP) else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, _offered) = Camera::start(&rig.stream, &rig.log, &rig.book);
    for generation in 1..=4 {
        rig.show_surface(generation);
        let skipped_before = skipped(&rig);
        assert!(
            wait_until(DEADLINE, || skipped(&rig) >= skipped_before + 5),
            "поколение {generation}: пропуски не пошли: {:?}",
            rig.handle.status()
        );
        // Surface уходит, пока (с большой вероятностью) придержана картинка пропущенного тика.
        rig.destroy_surface(generation);
        assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));
    }
    let status = rig.handle.status();
    assert_eq!(status.backend.validation_errors, 0, "{status:?}");
    assert_eq!(status.backend.detach_failures, 0);
    assert_eq!(status.backend_errors, 0, "{status:?}");
    // Каждая новая поверхность начинала без эталона: хотя бы по рисованному тику на поколение (новый кадр камеры или
    // тик без эталона), а пропуски шли на каждой.
    let counters = rig.viewfinder().counters.unchanged_frames;
    assert!(
        counters.drawn_no_baseline + counters.drawn_new_camera_frame + counters.drawn_not_quiet_live_recompose >= 4,
        "{counters:?}"
    );

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    assert_eq!(
        rig.context.live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    let counters = rig.stream.counters();
    assert_eq!((counters.leaked_leases, counters.frames_leaked), (0, 0), "{counters:?}");
}

/// Прогон 120 Гц / 60 к/с, пока показано `frames` кадров камеры. Возвращает (наибольшее число кадров на руках у
/// потока RAW, опубликованных статистик AWB на обработанный кадр, обработано кадров).
fn run_and_measure(policy: UnchangedFramePolicy, frames: u64) -> Option<(u64, f64, u64)> {
    let rig = start_rig_with_policy(policy)?;
    let (mut camera, epoch, _offered) = Camera::start(&rig.stream, &rig.log, &rig.book);
    rig.show_surface(1);
    let mut max_outstanding = 0;
    let started = std::time::Instant::now();
    while rig.camera_presented() < frames {
        max_outstanding = max_outstanding.max(rig.stream.counters().frames_outstanding);
        assert!(started.elapsed() < DEADLINE, "{:?}", rig.handle.status());
        std::thread::sleep(Duration::from_millis(2));
    }
    let viewfinder = rig.viewfinder();
    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    rig.assert_returns_were_safe(ReturnRules::Full);
    rig.destroy_surface(1);
    let processed = viewfinder.counters.frames_processed.max(1);
    Some((
        max_outstanding,
        viewfinder.counters.statistics_published as f64 / processed as f64,
        processed,
    ))
}

#[test]
fn skipping_keeps_camera_frames_on_hand_and_awb_statistics_at_the_level_of_redrawing_every_tick() {
    let Some((always_outstanding, always_statistics, _)) = run_and_measure(UnchangedFramePolicy::AlwaysRedraw, 90)
    else {
        return;
    };
    let Some((skip_outstanding, skip_statistics, processed)) = run_and_measure(SKIP, 90) else {
        return;
    };
    assert!(processed >= 90);
    assert!(
        skip_outstanding <= always_outstanding.max(1) + 1,
        "кадров камеры на руках при пропуске {skip_outstanding}, без пропуска {always_outstanding}"
    );
    assert!(
        skip_statistics >= always_statistics * 0.9,
        "статистик AWB на кадр при пропуске {skip_statistics}, без пропуска {always_statistics}"
    );
}
