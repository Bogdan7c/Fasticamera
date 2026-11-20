//! Кадрирование и увеличение при смене потока камеры (сессия 13, решения 111–113) на настоящем бэкенде показа
//! (RADV, headless, полная validation): кадр получает кадрирование СВОЕЙ камеры, а удержанный кадр
//! масштабируется за пальцем, который продолжает зумить. Стенд — `continuity_rig`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use camera_contract::fixtures::camera_id;
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::viewfinder_crop::{CropAspect, ZoomFactor};
use camera_contract::zoom::{
    LensGroup, LensZoomPoint, PreviewMirror, ZoomCatalog, ZoomFraming, ZoomRange, ZoomRatio, ZoomStop, ZoomStopKind,
};
use continuity_rig::{MAIN, Rig, SIZE_A, SIZE_B, WIDE, finish, gray, passport, reddish, test_continuity};
use render_loop::config::RevealExposureCheck;
use render_loop::viewfinder::{
    FrameLabel, FramingCrop, FramingDestination, TransitionKind, ViewfinderCommand, ViewfinderFraming,
};
fn lens(camera: &str, role: LensRole, native: f32, mirror: PreviewMirror) -> LensZoomPoint {
    LensZoomPoint {
        role,
        camera: camera_id(camera),
        native_ratio: ZoomRatio::new(native).expect("коэффициент"),
        equivalent_focal_mm: 24.0 / native,
        optical_axis: camera_contract::lens_calibration::OpticalAxis::Uncalibrated {
            reason: camera_contract::lens_calibration::UncalibratedReason::NotReported,
        },
        active_array: SIZE_A,
        preview_mirror: mirror,
        max_digital_zoom: ZoomFactor::new(10.0).expect("зум"),
    }
}

fn two_lens_catalog() -> Arc<ZoomCatalog> {
    let lenses = vec![
        lens(WIDE, LensRole::UltraWide, 0.59, PreviewMirror::Horizontal),
        lens(MAIN, LensRole::Wide, 1.0, PreviewMirror::None),
    ];
    let stops = lenses
        .iter()
        .map(|lens| ZoomStop {
            ratio: lens.native_ratio,
            kind: ZoomStopKind::Lens(lens.role),
        })
        .collect();
    Arc::new(ZoomCatalog {
        back: Some(LensGroup {
            facing: LensFacing::Back,
            lenses,
            stops,
            range: ZoomRange {
                min: ZoomRatio::new(0.59).expect("коэффициент"),
                max: ZoomRatio::new(10.0).expect("коэффициент"),
            },
        }),
        front: None,
    })
}

fn zoom_command(ratio: f32, catalog: &Arc<ZoomCatalog>) -> ViewfinderCommand {
    ViewfinderCommand::SetFraming(ViewfinderFraming {
        crop: FramingCrop::Zoom(ZoomFraming {
            aspect: CropAspect::SensorNative,
            facing: LensFacing::Back,
            ratio: ZoomRatio::new(ratio).expect("коэффициент"),
            catalog: Arc::clone(catalog),
        }),
        destination: FramingDestination::Configured,
    })
}

/// Кадрирование считается по КАМЕРЕ КАДРА: команда зума, пришедшая до первого кадра B, не даёт кадру A
/// кадрирование B (нет скачка на один кадр), а кадр B получает своё (зеркало, масштаб шкалы, показанный коэффициент).
/// Заодно: пока идёт удержание, а палец крутит зум дальше, удержанный кадр масштабируется за пальцем.
#[test]
fn each_frame_gets_the_crop_of_its_own_camera_and_the_held_picture_follows_a_finger_that_keeps_zooming() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::Ignore), false) else {
        return;
    };
    let catalog = two_lens_catalog();
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let passport_b = Arc::new(passport(WIDE, SIZE_B));

    // A (основная) показывается при зуме 2×; команда нового зума 3× приходит, пока идут кадры A.
    rig.command(zoom_command(2.0, &catalog));
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    rig.command(zoom_command(3.0, &catalog));
    rig.offer(epoch_a, gray());
    let a_tick = rig.tick();
    let a_crop = a_tick.record.crop.expect("кадр A обработан");
    assert_eq!(a_crop.camera, camera_id(MAIN));
    assert_eq!(a_crop.framed.crop.zoom.value(), 3.0, "основная: 3,0 / 1,0");
    assert_eq!(a_crop.framed.mirror, PreviewMirror::None);
    // Кадрирование реально сократило обработанную область: 3× от 64×48 = 21×16 → чётное 20×16.
    assert_eq!((a_crop.resolved.size.width, a_crop.resolved.size.height), (20, 16));

    // A закрылся, идёт удержание; палец довёл зум до 4,5×: удержанный кадр (показывал 3×) увеличен ×1,5.
    rig.stream.producer().end_stream(epoch_a);
    let first_hold = rig.tick();
    assert_eq!(
        first_hold.record.label,
        FrameLabel::Transition(TransitionKind::HeldLastFrame)
    );
    assert_eq!(
        first_hold.record.held_magnification, 1.0,
        "зум не менялся с момента показа: 3× → 3×"
    );
    rig.command(zoom_command(4.5, &catalog));
    let held_zoomed = rig.tick();
    assert!(
        (held_zoomed.record.held_magnification - 1.5).abs() < 1e-5,
        "увеличение = желаемое 4,5 / показанное 3,0: {}",
        held_zoomed.record.held_magnification
    );
    // Зум назад ниже показанного: увеличения нет (чёрных полей не бывает).
    rig.command(zoom_command(2.0, &catalog));
    let held_back = rig.tick();
    assert_eq!(held_back.record.held_magnification, 1.0);

    // B (ширик, зеркальный) стартует при желаемых 4,5×: его кадр получает кадрирование ШИРИКА.
    rig.command(zoom_command(4.5, &catalog));
    let epoch_b = rig.begin(&passport_b);
    let b_ticks = rig.feed_until_live(epoch_b, reddish());
    let b_crop = b_ticks
        .last()
        .and_then(|tick| tick.record.crop.clone())
        .expect("кадр B обработан");
    assert_eq!(b_crop.camera, camera_id(WIDE));
    assert!(
        (b_crop.framed.crop.zoom.value() - 4.5 / 0.59).abs() < 1e-4,
        "ширик: 4,5 / 0,59, а не 3,0 основной: {}",
        b_crop.framed.crop.zoom.value()
    );
    assert_eq!(b_crop.framed.mirror, PreviewMirror::Horizontal);
    assert!((b_crop.framed.shown_ratio.value() - 4.5).abs() < 1e-4);

    let counters = rig.counters();
    assert_eq!(counters.black_frames_while_picture_expected, 0, "{counters:?}");
    finish(&mut rig, &[epoch_b]);
}
