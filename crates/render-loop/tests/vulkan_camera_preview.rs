//! Функциональный тест показа кадров камеры: НАСТОЯЩИЙ поток рендера + `VulkanDisplayBackend` в режиме
//! камеры на реальном драйвере (RADV, headless-показ, полная validation) + настоящий `RawFrameStream` и
//! поддельный производитель. Импорт буфера в GPU подменён тестовым импортёром (на ПК нет
//! `AHardwareBuffer`): он создаёт обычные картинки Vulkan с известным RAW-узором. Всё остальное — боевой
//! путь: опрос видоискателя, ISP видоискателя (сессия 07), компоновка, сабмит с ожиданием/экспортом
//! fence, учёт лизингов, возврат камере, статистика AWB.
//!
//! Доказывается: кадры идут производитель -> поток рендера -> ISP -> показ; каждый кадр импортируется
//! и читается РОВНО одним сабмитом и возвращается производителю с настоящим fence этого сабмита (на
//! руках у показа не больше одного кадра); между кадрами камеры экран собирается из прежнего
//! изображения без повторного импорта; команды видоискателю переключают рисовальщик и стадии на ходу;
//! после остановки и отсоединения возвращено всё, `leaked_leases == 0`, объекты Vulkan вернулись к
//! исходным, validation чиста.
//!
//! Цвета выхода ISP через настоящий бэкенд проверяет `vulkan_viewfinder_content.rs`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod camera_rig;
mod support;
mod vulkan_support;
use camera_rig::*;
use isp_vulkan::viewfinder::{IspStage, IspStageSet};
use render_loop::config::UnchangedFramePolicy;
use render_loop::thread::RenderState;
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome, ViewfinderPainter};
use std::sync::atomic::Ordering;
use std::time::Duration;
use support::wait_until;
use vulkan_support::assert_no_validation_errors;

#[test]
fn frames_flow_to_present_and_every_lease_returns_safely() {
    frames_flow_and_leases_return(UnchangedFramePolicy::AlwaysRedraw);
}

/// Второй режим ключевого стенда: те же проверки (возвраты лизингов безопасны, на руках не больше одного кадра, статистика
/// AE-статистика от рисовавшегося кадра, ничего не утекло) при включённом пропуске тиков без изменений. Вместо счётчика
/// перекомпоновок — счётчик пропущенных тиков: между кадрами камеры 60 к/с на экране 120 Гц тики пропускаются.
#[test]
fn frames_flow_and_every_lease_returns_safely_while_unchanged_ticks_are_skipped() {
    frames_flow_and_leases_return(UnchangedFramePolicy::SkipWhenUnchanged);
}

fn frames_flow_and_leases_return(policy: UnchangedFramePolicy) {
    let Some(rig) = start_rig_with_auto_exposure(policy) else {
        return;
    };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);

    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.camera_presented() >= 30),
        "кадры камеры не дошли до показа: {:?}",
        rig.handle.status()
    );
    let first_sequence = rig
        .handle
        .status()
        .backend
        .camera
        .and_then(|camera| camera.last_presented_frame)
        .expect("последний показанный кадр известен")
        .sequence;
    assert!(
        wait_until(DEADLINE, || {
            rig.handle
                .status()
                .backend
                .camera
                .and_then(|camera| camera.last_presented_frame)
                .is_some_and(|frame| frame.sequence > first_sequence + 5)
        }),
        "последовательность показанных кадров не растёт"
    );

    // Пока идёт показ: лизинги уже возвращаются (по fence более позднего сабмита), камера не голодает.
    assert!(
        wait_until(DEADLINE, || rig.book.returned_count() >= 10),
        "производитель не получает буферы назад: {:?}",
        rig.handle.status().backend.camera
    );
    // Камера 60 к/с, экран 120 Гц: между кадрами камеры экран собирается из прежнего изображения.
    match policy {
        UnchangedFramePolicy::AlwaysRedraw => assert!(
            wait_until(DEADLINE, || rig.viewfinder().counters.recomposed >= 10),
            "перекомпоновки без нового кадра не было: {:?}",
            rig.viewfinder()
        ),
        UnchangedFramePolicy::SkipWhenUnchanged => {
            assert!(
                wait_until(DEADLINE, || rig.handle.status().ticks_skipped_unchanged >= 10),
                "тики без изменений не пропускаются: {:?}",
                rig.viewfinder().counters.unchanged_frames
            );
            assert_eq!(
                rig.viewfinder().counters.recomposed,
                0,
                "перекомпоновка, которой нет, не рисуется"
            );
        }
    }
    assert!(
        wait_until(DEADLINE, || rig
            .viewfinder()
            .gpu_time
            .isp_dispatch
            .is_some_and(|window| window.samples > 0)),
        "время GPU прохода ISP не замерено: {:?}",
        rig.viewfinder()
    );
    let streaming = rig.handle.status();
    let diagnostics = streaming.backend.camera.clone().expect("диагностика камеры");
    let viewfinder = rig.viewfinder();
    assert_eq!(viewfinder.painter, ViewfinderPainter::Isp, "по умолчанию — ISP");
    assert_eq!(viewfinder.stages, IspStageSet::ALL);
    assert!(
        viewfinder.counters.frames_processed >= diagnostics.camera_frames_presented,
        "каждый показанный кадр прошёл через ISP: {viewfinder:?} / {diagnostics:?}"
    );
    assert!(viewfinder.counters.params_resolved >= 1, "{viewfinder:?}");
    assert_eq!(viewfinder.counters.params_failed, 0, "{viewfinder:?}");
    assert_eq!(viewfinder.counters.isp_record_failures, 0, "{viewfinder:?}");
    assert_eq!(viewfinder.counters.compose_failures, 0, "{viewfinder:?}");
    assert_eq!(viewfinder.counters.cpu_copies, 0);
    // Кэш вариантов пайплайна доходит до диагностики: первый кадр создал свой вариант и заготовил остальные,
    // а дальше (кадры идут, статистика чередуется) в кадрах ничего не создаётся и не вытесняется.
    let variants = viewfinder.pipeline_variants;
    assert!(
        variants.created_in_frame == 1 && variants.created_by_prewarm >= 1,
        "{variants:?}"
    );
    assert!(variants.in_cache >= 2, "{variants:?}");
    let frames_now = viewfinder.counters.frames_processed;
    assert!(
        wait_until(DEADLINE, || rig.viewfinder().counters.frames_processed
            >= frames_now + 30),
        "кадры перестали идти: {:?}",
        rig.viewfinder()
    );
    let later = rig.viewfinder().pipeline_variants;
    assert_eq!(
        (later.created_in_frame, later.evicted, later.create_failed_in_frame),
        (variants.created_in_frame, 0, 0),
        "после прогрева кадры создают или вытесняют пайплайны: было {variants:?}, стало {later:?}"
    );
    assert!(
        viewfinder.output_extent.is_some() && viewfinder.storage.is_some(),
        "{viewfinder:?}"
    );
    let frame_time = viewfinder.gpu_time.frame_total.expect("время кадра видоискателя");
    assert!(
        frame_time.samples > 0 && frame_time.max > Duration::ZERO,
        "{frame_time:?}"
    );
    // P13: время кадра на GPU доходит из бэкенда до потока рендера и входит в оценку стоимости кадра.
    assert!(
        wait_until(DEADLINE, || rig
            .handle
            .status()
            .present_target
            .frame_gpu_time
            .is_some_and(|window| window.samples >= 30)),
        "время кадра на GPU не дошло до потока рендера: {:?}",
        rig.handle.status().present_target
    );
    let cost = rig.handle.status().present_target.estimated_cost_parts;
    assert_eq!(
        cost.gpu_source,
        render_loop::thread::GpuCostSource::Measured,
        "{cost:?}"
    );
    assert!(
        cost.gpu > Duration::ZERO && cost.gpu <= frame_time.max.max(Duration::from_millis(50)),
        "{cost:?}"
    );
    // Статистика AWB опубликована и привязана к кадру, который действительно рисовался.
    let published = rig.awb.latest().expect("статистика AWB опубликована");
    assert!(viewfinder.counters.statistics_published > 0);
    assert!(
        rig.log
            .snapshot()
            .iter()
            .any(|event| matches!(event, Event::Draw { sensor_ts, .. } if *sensor_ts == published.frame.sensor_timestamp_nanos)),
        "статистика AWB от кадра, который не рисовался: {:?}",
        published.frame
    );
    assert!(published.frame.frame_number.is_some(), "у кадра были метаданные");
    rig.assert_streaming_returns_carry_real_fences(rig.book.returned_count());
    assert_eq!(streaming.state, RenderState::Presenting);
    assert_eq!(streaming.backend_errors, 0, "{streaming:?}");
    assert!(diagnostics.camera_frames_imported >= diagnostics.camera_frames_presented);
    assert!(
        diagnostics.import_cache_misses >= 30,
        "каждый новый кадр — настоящий импорт: {diagnostics:?}"
    );
    assert!(diagnostics.release_fences_exported > 0, "{diagnostics:?}");
    assert_eq!(diagnostics.release_export_failures, 0, "{diagnostics:?}");
    assert!(
        diagnostics.present_history.len() >= 2
            && diagnostics
                .present_history
                .windows(2)
                .all(|pair| pair[0].sensor_timestamp_nanos < pair[1].sensor_timestamp_nanos),
        "история показов: метки сенсора растут: {:?}",
        diagnostics.present_history
    );
    rig.assert_returns_were_safe(ReturnRules::Full);

    // Остановка камеры: поток рендера видит StreamInactive, рисует чёрный и возвращает показанный кадр.
    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || rig.book.returned_count() == offered_total),
        "после остановки возвращено {} из {offered_total}: {:?}",
        rig.book.returned_count(),
        rig.stream.counters()
    );
    assert!(
        wait_until(DEADLINE, || rig.imports_alive.load(Ordering::SeqCst) == 0),
        "импорты не освобождены после остановки потока"
    );
    let presented_after_stop = rig.camera_presented();

    rig.destroy_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));
    let counters = rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_leaked, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_eq!(
        rig.camera_presented(),
        presented_after_stop,
        "после остановки новые кадры не показываются"
    );
    rig.assert_returns_were_safe(ReturnRules::Full);
    assert_eq!(
        rig.context.live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    let final_status = rig.handle.status();
    assert_eq!(final_status.backend.validation_errors, 0);
    assert_eq!(final_status.backend.detach_failures, 0);
    assert_no_validation_errors(&rig.context);
}

#[test]
fn destroying_the_surface_mid_stream_returns_every_lease_without_leaks() {
    destroy_surface_mid_stream(ViewfinderPainter::Isp);
}

#[test]
fn destroying_the_surface_mid_stream_with_the_debug_mosaic_returns_the_displayed_frame_after_idle() {
    destroy_surface_mid_stream(ViewfinderPainter::DebugMosaic);
}

/// Surface уничтожается посреди показа выбранным рисовальщиком: всё возвращено, ничего не утекло.
fn destroy_surface_mid_stream(painter: ViewfinderPainter) {
    let Some(rig) = start_rig() else { return };
    let baseline = rig.context.live_objects();
    rig.set_painter(painter);
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);

    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.camera_presented() >= 15),
        "кадры не дошли до показа"
    );
    // Surface уничтожается, пока камера продолжает слать кадры.
    rig.destroy_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || rig.book.returned_count() == offered_total),
        "возвращено {} из {offered_total}: {:?}",
        rig.book.returned_count(),
        rig.stream.counters()
    );
    let counters = rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    let final_status = rig.handle.status();
    let camera_diagnostics = final_status.backend.camera.expect("диагностика камеры");
    match painter {
        // ISP: кадр возвращается своим же сабмитом — к отсоединению на руках у показа нет ни одного.
        ViewfinderPainter::Isp => {
            assert_eq!(
                camera_diagnostics.leases_released_after_idle, 0,
                "{camera_diagnostics:?}"
            );
            rig.assert_returns_were_safe(ReturnRules::Full);
        }
        // Мозаика держит показанный кадр до замены: при отсоединении он возвращается по простою GPU.
        ViewfinderPainter::DebugMosaic => {
            assert!(
                camera_diagnostics.leases_released_after_idle >= 1,
                "показанный кадр возвращён по простою GPU при отсоединении: {camera_diagnostics:?}"
            );
            rig.assert_returns_were_safe(ReturnRules::DebugMosaic);
        }
    }
    assert_eq!(
        camera_diagnostics.leases_leaked_on_teardown, 0,
        "{camera_diagnostics:?}"
    );
    assert_eq!(rig.context.live_objects(), baseline);
    assert_eq!(final_status.backend.validation_errors, 0);
    assert_no_validation_errors(&rig.context);
}

#[test]
fn viewfinder_commands_switch_painter_and_stages_while_streaming() {
    let Some(rig) = start_rig_with_auto_exposure(default_unchanged_policy()) else {
        return;
    };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);
    rig.show_surface(1);
    assert!(wait_until(DEADLINE, || rig.viewfinder().counters.frames_processed >= 10));

    // 1. Без статистики AWB: проход идёт, публикации прекращаются.
    let without_statistics = IspStageSet::ALL.without(IspStage::AwbStatistics);
    rig.command(
        ViewfinderCommand::SetIspStages(without_statistics),
        ViewfinderCommandOutcome::Applied,
    );
    assert!(wait_until(DEADLINE, || rig.viewfinder().stages == without_statistics));
    let processed = rig.viewfinder().counters.frames_processed;
    assert!(wait_until(DEADLINE, || rig.viewfinder().counters.frames_processed >= processed + 5));
    let published = rig.awb.publications();
    let processed = rig.viewfinder().counters.frames_processed;
    assert!(wait_until(DEADLINE, || rig.viewfinder().counters.frames_processed
        >= processed + 10));
    assert_eq!(
        rig.awb.publications(),
        published,
        "выключенная стадия статистики ничего не публикует"
    );
    rig.command(
        ViewfinderCommand::SetIspStages(without_statistics),
        ViewfinderCommandOutcome::AlreadyInEffect,
    );

    // 2. Отладочная мозаика: ISP больше не работает, кадры показываются.
    rig.command(
        ViewfinderCommand::SetPainter(ViewfinderPainter::DebugMosaic),
        ViewfinderCommandOutcome::Applied,
    );
    assert!(wait_until(DEADLINE, || rig.viewfinder().painter == ViewfinderPainter::DebugMosaic));
    let processed = rig.viewfinder().counters.frames_processed;
    let presented = rig.camera_presented();
    assert!(wait_until(DEADLINE, || rig.camera_presented() >= presented + 15));
    assert_eq!(
        rig.viewfinder().counters.frames_processed,
        processed,
        "мозаика рисует без ISP"
    );

    // 3. Назад на ISP со всеми стадиями: проход и статистика возобновляются.
    rig.command(
        ViewfinderCommand::SetPainter(ViewfinderPainter::Isp),
        ViewfinderCommandOutcome::Applied,
    );
    rig.command(
        ViewfinderCommand::SetIspStages(IspStageSet::ALL),
        ViewfinderCommandOutcome::Applied,
    );
    assert!(wait_until(DEADLINE, || rig.viewfinder().counters.frames_processed
        >= processed + 10));
    assert!(wait_until(DEADLINE, || rig.awb.publications() > published));
    let commands = rig.handle.status().viewfinder_commands;
    assert_eq!((commands.applied, commands.already_in_effect), (4, 1), "{commands:?}");

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(wait_until(DEADLINE, || rig.book.returned_count() == offered_total));
    rig.destroy_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));
    assert_eq!(rig.stream.counters().leaked_leases, 0);
    rig.assert_returns_were_safe(ReturnRules::NoReadAfterReturn);
    assert_eq!(rig.context.live_objects(), baseline);
    assert_no_validation_errors(&rig.context);
}

#[test]
fn surface_cycles_keep_counters_cumulative_and_leak_nothing() {
    let Some(rig) = start_rig() else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);

    let mut presented_before = 0;
    for generation in 1..=3 {
        rig.show_surface(generation);
        assert!(
            wait_until(DEADLINE, || rig.camera_presented() >= presented_before + 8),
            "цикл {generation}: кадры не дошли до показа ({presented_before} уже было)"
        );
        presented_before = rig.camera_presented();
        rig.destroy_surface(generation);
        assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));
        assert!(
            rig.camera_presented() >= presented_before,
            "счётчики показа не обнуляются при отсоединении surface"
        );
    }

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(wait_until(DEADLINE, || rig.book.returned_count() == offered_total));
    assert_eq!(rig.stream.counters().leaked_leases, 0);
    rig.assert_returns_were_safe(ReturnRules::NoReadAfterReturn);
    assert_eq!(rig.context.live_objects(), baseline);
    assert_no_validation_errors(&rig.context);
}
