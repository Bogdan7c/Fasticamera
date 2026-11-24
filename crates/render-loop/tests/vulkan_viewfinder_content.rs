//! Цвета выхода ISP через НАСТОЯЩИЙ бэкенд показа: `VulkanDisplayBackend` в режиме камеры на реальном
//! драйвере (RADV, headless-показ, полная validation) + настоящий `RawFrameStream`. Синтетический RAW
//! (каждая ячейка 2×2 одинакова) проходит боевой путь — опрос, параметры ISP из паспорта и
//! подсказок кадра, импорт, ISP, компоновка, возврат лизинга, — и изображение видоискателя читается
//! обратно (тестовый шов `read_viewfinder_image`).
//!
//! Доказывается: цвет каждого пикселя изображения совпадает с эталоном `isp-model` на ТЕХ ЖЕ входах
//! (паспорт, нейтраль HAL, гамма экрана); «серый под нейтраль» выходит серым; цветной кадр сменяет
//! серый (изображение действительно обновляется новым кадром); после остановки потока и отсоединения
//! все лизинги возвращены, объекты Vulkan вернулись к исходным, validation чиста.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod viewfinder_rig;
mod vulkan_support;

use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use camera_contract::three_a::DirectiveSlot;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use raw_frame_stream::{RawFrameStream, RawFrameStreamConfig};
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig, ViewfinderConfig, ViewfinderToneConfig};
use support::{Journal, fake_surface};
use viewfinder_rig::*;
use vulkan_support::{
    assert_no_validation_errors, assert_presented_or_skipped, headless_camera_backend_with_directives_or_skip,
};

#[test]
fn isp_output_through_the_real_backend_matches_the_isp_model_reference() {
    let render_config = RenderLoopConfig {
        // Headless-стенд и так даёт sRGB; явный выбор делает эталон однозначным.
        display_color: DisplayColorConfig::SrgbOnly,
        ..RenderLoopConfig::default()
    };
    // Тест проверяет путь кадра «первый же vsync», а не политику видоискателя: `LatestWins`
    // выдаёт одиночный кадр сразу (буфер с запасом ждал бы второй — см. raw-frame-stream и стенд).
    // Кольцо снимков выключено осознанно: тест про возврат кадров видоискателя, а кольцо (по умолчанию 12 кадров,
    // решение 139) держит доли кадров, и камера получает их назад лишь при вытеснении. Кольцо проверяют `vulkan_still_*.rs`.
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        snapshot_ring_max_frames: 0,
        ..RawFrameStreamConfig::default()
    });
    let cells = HashMap::from([(GRAY_TS, neutral_gray()), (RED_TS, reddish())]);
    let directives = DirectiveSlot::new();
    directives.publish(manual_white_directive());
    let Some(mut backend) = headless_camera_backend_with_directives_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(UniformImporter {
                context: context.clone(),
                cells: cells.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        directives,
    ) else {
        return;
    };
    let baseline = backend.context().live_objects();
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let passport = Arc::new(stream_passport());
    let epoch = stream.producer().begin_stream(Arc::clone(&passport)).epoch;
    let returned = Arc::new(Returned::default());

    // 1. Серый под нейтраль.
    offer(&stream, epoch, GRAY_TS, 1, &returned);
    let mut frame_index = present_until_processed(&mut backend, 0, 1);
    let gray = reference_color(&passport, neutral_gray());
    let spread = gray.iter().copied().fold(f32::MIN, f32::max) - gray.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        spread < 0.01 && gray[1] > 0.2,
        "эталон серого — серый средней яркости: {gray:?}"
    );
    assert_image_color(&backend, gray, "серый");

    // 2. Красный кадр сменяет серый: изображение обновилось новым кадром.
    offer(&stream, epoch, RED_TS, 2, &returned);
    frame_index = present_until_processed(&mut backend, frame_index, 2);
    let red = reference_color(&passport, reddish());
    // Основа AgX с возвратом оттенка (решение 180) держит насыщенный красный красным: без возврата он уходил в
    // пурпур (синий выше зелёного), поэтому условие — красный заметно выше ОБОИХ других каналов.
    assert!(
        red[0] > red[1] + 0.2 && red[0] > red[2] + 0.2,
        "эталон красного — красный: {red:?}"
    );
    assert_image_color(&backend, red, "красный");

    // Оба кадра возвращены производителю во время показа (каждый — своим сабмитом).
    assert_eq!(
        *returned.0.lock().unwrap_or_else(PoisonError::into_inner),
        vec![GRAY_TS, RED_TS],
        "кадры ISP возвращаются сразу, а не ждут следующего кадра"
    );

    // 3. Остановка потока: чёрный кадр, затем отсоединение без утечек.
    stream.producer().end_stream(epoch);
    assert_presented_or_skipped(backend.render_and_present(timing(frame_index)));
    let viewfinder = backend.diagnostics().viewfinder.expect("диагностика видоискателя");
    assert_eq!(
        viewfinder.counters.params_resolved, 1,
        "входы не менялись: параметры посчитаны один раз"
    );
    assert!(viewfinder.counters.recomposed >= 2, "{viewfinder:?}");
    assert_eq!(viewfinder.counters.isp_record_failures, 0, "{viewfinder:?}");
    assert!(backend.detach().is_some(), "окно возвращено");
    let counters = stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_eq!(
        backend.context().live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    assert_no_validation_errors(backend.context());
}

/// Ручки тона из конфига (`viewfinder.tone`, сессия 12b) доходят до показанного кадра: серый с контрастом 1,8 и
/// серединой экрана 0,24 совпадает с эталоном `isp-model` с ТЕМИ ЖЕ ручками и заметно отличается от серого по
/// умолчанию. Если конфиг до кэша параметров не дойдёт, ISP нарисует серый по умолчанию и тест упадёт.
#[test]
fn tone_knobs_from_the_config_reach_the_shown_viewfinder_image() {
    let tone = ViewfinderToneConfig {
        contrast: 1.8,
        display_middle_gray: 0.24,
        ..ViewfinderToneConfig::default()
    };
    let render_config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            tone,
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    render_config
        .validate()
        .expect("конфиг с ручками тона проходит проверку");
    // Кольцо снимков выключено осознанно: тест про возврат кадров видоискателя, а кольцо (по умолчанию 12 кадров,
    // решение 139) держит доли кадров, и камера получает их назад лишь при вытеснении. Кольцо проверяют `vulkan_still_*.rs`.
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        snapshot_ring_max_frames: 0,
        ..RawFrameStreamConfig::default()
    });
    let cells = HashMap::from([(GRAY_TS, neutral_gray())]);
    let directives = DirectiveSlot::new();
    directives.publish(manual_white_directive());
    let Some(mut backend) = headless_camera_backend_with_directives_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(UniformImporter {
                context: context.clone(),
                cells: cells.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        directives,
    ) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let passport = Arc::new(stream_passport());
    let epoch = stream.producer().begin_stream(Arc::clone(&passport)).epoch;
    let returned = Arc::new(Returned::default());

    offer(&stream, epoch, GRAY_TS, 1, &returned);
    present_until_processed(&mut backend, 0, 1);

    let configured = reference_color_with_tone(&passport, neutral_gray(), &tone);
    let default = reference_color(&passport, neutral_gray());
    // Эталоны должны различаться сильнее допуска сравнения, иначе тест не отличил бы «дошло» от «не дошло».
    let separation = (configured[1] - default[1]).abs();
    assert!(
        separation > 4.0 * TOLERANCE,
        "ручки тона почти не меняют серый ({configured:?} против {default:?}): тест ничего не докажет"
    );
    assert_image_color(&backend, configured, "серый с ручками тона из конфига");

    stream.producer().end_stream(epoch);
    assert!(backend.detach().is_some(), "окно возвращено");
    assert_eq!(stream.counters().leaked_leases, 0);
    assert_no_validation_errors(backend.context());
}
