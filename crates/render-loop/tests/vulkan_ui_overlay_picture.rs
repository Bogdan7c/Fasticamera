//! Картинка UI через НАСТОЯЩИЙ бэкенд показа (сессия 14g): источник кадра UI отдаёт список с `ImageItem`,
//! рендер загружает пиксели в текстуру и рисует кадр, отсоединение и новая surface не теряют картинку.
//!
//! Цель показа (swapchain) на headless-стенде прочитать нельзя, а диагностика бэкенда не несёт счётчиков
//! картинок рендера, поэтому «картинка нарисована заново» доказывается тремя наблюдаемыми вещами:
//! * счётчики кадров UI растут, отказов нет (`last_error` пуст);
//! * число живых объектов Vulkan: появление картинки добавляет ровно текстуру (картинка + представление +
//!   память), staging после fence-ей ушёл;
//! * при отсоединении surface рендер UI уничтожается вместе с цепочкой — текстура исчезает, а после новой
//!   surface появляется снова (то есть загружена заново из картинки в памяти).
//!
//! Попиксельная сверка цвета картинки — в `ui-render-vulkan/tests/images.rs` (там цель можно читать).

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

use gpu_vulkan::live_objects::LiveObjectKind;
use render_loop::config::UiOverlayConfig;
use ui_overlay_rig::*;

/// Сколько кадров показа гоняем, чтобы fence-ы слотов прошли и staging освободился.
const SETTLE_FRAMES: u64 = 8;

/// Прирост числа объектов, из которых состоят текстура и staging картинки: `[Image, ImageView, DeviceMemory, Buffer]`.
/// Прочие виды (например, fence-ы потока кадров) к картинкам не относятся и в сравнение не входят.
fn picture_object_growth(
    with: &gpu_vulkan::live_objects::LiveObjectSnapshot,
    without: &gpu_vulkan::live_objects::LiveObjectSnapshot,
) -> [isize; 4] {
    [
        LiveObjectKind::Image,
        LiveObjectKind::ImageView,
        LiveObjectKind::DeviceMemory,
        LiveObjectKind::Buffer,
    ]
    .map(|kind| with.count(kind) as isize - without.count(kind) as isize)
}

#[test]
fn a_picture_is_uploaded_and_drawn_and_drawn_again_on_a_new_surface() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let probe = PictureProbe::new();
    harness.install_pictures(&probe);

    // Источник жив, но картинки в списке нет: рендер UI создан, текстур картинок нет.
    harness.show_new_camera_frame();
    harness.recompose(SETTLE_FRAMES);
    let without_picture = harness.live_objects();
    let drawn_without = harness.ui().frames_drawn;

    // Картинка появилась: загрузка в кадре, дальше кадры берут текстуру из кэша.
    probe.show(Some(test_picture(24, 16)));
    harness.show_new_camera_frame();
    harness.recompose(SETTLE_FRAMES);
    let with_picture = harness.live_objects();
    let ui = harness.ui();
    assert!(ui.frames_drawn > drawn_without, "{ui:?}");
    assert_eq!(
        (ui.frames_failed, ui.source_failures, ui.last_error.clone()),
        (0, 0, None),
        "{ui:?}"
    );
    assert_eq!(
        picture_object_growth(&with_picture, &without_picture),
        [1, 1, 1, 0],
        "картинка добавила ровно одну текстуру (image + view + память); staging-буфер после fence-ей освобождён"
    );

    // Surface ушла: рендер UI разобран вместе с цепочкой, текстура картинки уничтожена.
    harness.detach();
    let detached = harness.live_objects();
    assert!(
        detached.count(LiveObjectKind::Image) < with_picture.count(LiveObjectKind::Image),
        "текстура картинки ушла вместе с рендером: {detached} против {with_picture}"
    );
    let drawn_before_reattach = harness.ui().frames_drawn;

    // Новая surface: источник тот же, картинка в памяти та же — рендер создан заново и загрузил её снова.
    harness.attach(2, SURFACE);
    harness.show_new_camera_frame();
    harness.recompose(SETTLE_FRAMES);
    let reattached = harness.ui();
    assert!(reattached.frames_drawn > drawn_before_reattach, "{reattached:?}");
    assert_eq!(
        (
            reattached.frames_failed,
            reattached.source_failures,
            reattached.last_error.clone()
        ),
        (0, 0, None),
        "{reattached:?}"
    );
    assert_eq!(
        harness.live_objects().count(LiveObjectKind::Image),
        with_picture.count(LiveObjectKind::Image),
        "после новой surface текстура картинки снова на месте"
    );
    assert_eq!(reattached.installs, 1, "источник не переустанавливался");
    harness.finish();
}

#[test]
fn an_oversized_picture_fails_the_ui_frame_honestly_and_allocates_nothing_while_the_camera_keeps_running() {
    let Some(mut harness) = Harness::start(UiOverlayConfig::default()) else {
        return;
    };
    harness.attach(1, SURFACE);
    harness.show_new_camera_frame();
    let probe = PictureProbe::new();
    harness.install_pictures(&probe);
    harness.show_new_camera_frame();
    harness.recompose(SETTLE_FRAMES);
    let before = harness.live_objects();
    let failed_before = harness.ui().frames_failed;
    let processed_before = harness.viewfinder().counters.frames_processed;

    // Предел стороны по умолчанию — 1024 px: картинка 1100 × 4 отвергается до выделения памяти.
    probe.show(Some(test_picture(1100, 4)));
    harness.show_new_camera_frame();
    harness.recompose(2);
    let ui = harness.ui();
    assert!(
        ui.frames_failed >= failed_before + 3,
        "каждый кадр с картинкой отвергнут: {ui:?}"
    );
    let error = ui.last_error.clone().expect("причина сохранена");
    assert!(error.contains("1024"), "в причине указан предел: {error}");
    // Сравниваются только виды, из которых состоит картинка: пул fence-ов показа растёт лениво по
    // таймингу движка показа (`PresentFencePool::begin_present`) и к картинке не относится — полный
    // снимок давал плавающее падение (≈ 1 из 40 прогонов и до 14h).
    assert_eq!(
        picture_object_growth(&harness.live_objects(), &before),
        [0; 4],
        "память под отвергнутую картинку не выделялась"
    );
    assert!(
        harness.viewfinder().counters.frames_processed > processed_before,
        "видоискатель идёт, пока UI отказывает"
    );

    // Нормальная картинка возвращает UI к жизни без пересоздания чего-либо.
    probe.show(Some(test_picture(24, 16)));
    let drawn_before = harness.ui().frames_drawn;
    harness.recompose(2);
    assert!(harness.ui().frames_drawn >= drawn_before + 2);
    harness.finish();
}
