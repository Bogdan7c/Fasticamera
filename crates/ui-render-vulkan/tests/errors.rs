//! Ошибки кадра: каждая причина различима, цель и пирамида не портятся, счётчики честные.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::{
    ColorTarget, ColorTargetDesc, ColorTargetFormat, ColorTargetPreviousUse, ContentOnPrepare,
};
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::raster::{AttachmentLoad, RasterPass};
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::{
    CLEAR, Rig, assert_no_validation_errors, empty_list, glass_group, gpu_context_or_skip, lens_only_style, shape,
    shape_item,
};
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawListError, IconItem};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::icons::IconId;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;
use ui_render_vulkan::{BackdropInput, LimitKind, UiRenderError};

const ABSENT: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor::BLACK,
};

fn square() -> ShapeGeometry {
    ShapeGeometry::RoundedRect {
        size: PxSize::new(10.0, 10.0),
        corner: CornerSpec::circular(2.0),
    }
}

fn assert_target_untouched(pixels: &[[f32; 4]]) {
    assert!(
        pixels.iter().all(|p| (0..3).all(|c| (p[c] - CLEAR[c]).abs() < 0.005)),
        "цель испорчена отвергнутым кадром"
    );
}

#[test]
fn exceeding_each_limit_is_an_honest_frame_error_with_counters_and_an_untouched_target() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut rig = Rig::with_config(
        &context,
        (100, 60),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
        |config| {
            config.limits.max_shape_instances = 3;
            config.limits.max_glyph_instances = 2;
            config.limits.max_glass_groups = 1;
        },
    );

    let mut too_many_shapes = empty_list();
    for index in 0..4 {
        too_many_shapes.items.push(shape(shape_item(
            square(),
            Affine2::translate(15.0 + index as f32 * 20.0, 30.0),
            DisplayColor::WHITE,
        )));
    }
    let (result, pixels) = rig.render_with_clear(&too_many_shapes, ABSENT, CLEAR);
    assert!(
        matches!(
            result,
            Err(UiRenderError::LimitExceeded {
                kind: LimitKind::ShapeInstances,
                requested: 4,
                limit: 3
            })
        ),
        "{result:?}"
    );
    assert_target_untouched(&pixels);

    let mut too_many_glyphs = empty_list();
    for index in 0..3 {
        too_many_glyphs.items.push(DrawItem::Icon(IconItem {
            icon: IconId::Grid,
            transform: Affine2::translate(15.0 + index as f32 * 25.0, 30.0),
            size_px: PxSize::new(16.0, 16.0),
            color: DisplayColor::WHITE,
            opacity: ui_draw::color::Opacity::OPAQUE,
        }));
    }
    let (result, pixels) = rig.render_with_clear(&too_many_glyphs, ABSENT, CLEAR);
    assert!(
        matches!(
            result,
            Err(UiRenderError::LimitExceeded {
                kind: LimitKind::GlyphInstances,
                requested: 3,
                limit: 2
            })
        ),
        "{result:?}"
    );
    assert_target_untouched(&pixels);

    let mut too_many_groups = empty_list();
    for index in 0..2 {
        too_many_groups.items.push(DrawItem::Glass(glass_group(
            &lens_only_style(),
            &[(square(), PxPoint::new(20.0 + index as f32 * 30.0, 30.0))],
            0.0,
        )));
    }
    let (result, pixels) = rig.render_with_clear(&too_many_groups, ABSENT, CLEAR);
    assert!(
        matches!(
            result,
            Err(UiRenderError::LimitExceeded {
                kind: LimitKind::GlassGroups,
                requested: 2,
                limit: 1
            })
        ),
        "{result:?}"
    );
    assert_target_untouched(&pixels);

    let counters = rig.renderer.counters();
    assert_eq!(
        (
            counters.frames_failed,
            counters.limit_overflows,
            counters.frames_recorded
        ),
        (3, 3, 0)
    );
    assert_eq!(
        (counters.pyramid_rebuilds, counters.pyramid_recreations),
        (0, 0),
        "отвергнутый кадр пирамиду не трогал"
    );

    // Ровно на пределе — кадр проходит (граница включена) и рисует.
    let mut at_limit = empty_list();
    for index in 0..3 {
        at_limit.items.push(shape(shape_item(
            square(),
            Affine2::translate(15.0 + index as f32 * 20.0, 30.0),
            DisplayColor::WHITE,
        )));
    }
    let (report, pixels) = rig.render(&at_limit, ABSENT);
    assert_eq!(report.shape_instances, 3);
    assert!(pixels[(30 * 100 + 15) as usize][0] > 0.9);
    drop((rig, too_many_shapes, too_many_glyphs, too_many_groups, at_limit));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn misuse_errors_are_distinguished_and_leave_the_target_untouched() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::new(
        &context,
        (100, 60),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let mut list = empty_list();
    list.items.push(shape(shape_item(
        square(),
        Affine2::translate(50.0, 30.0),
        DisplayColor::WHITE,
    )));

    // Все попытки записываются в один командный буфер поверх очищенной цели; отвергнутые попытки ничего не рисуют (рисует только последняя успешная — слот 1).
    let other_format = ColorTarget::new(
        &context,
        &ColorTargetDesc {
            extent: ash::vk::Extent2D { width: 100, height: 60 },
            format: ColorTargetFormat::Rgba16Float,
            label: "other",
        },
    )
    .unwrap();
    let wrong_size = ColorTarget::new(
        &context,
        &ColorTargetDesc {
            extent: ash::vk::Extent2D { width: 64, height: 64 },
            format: ColorTargetFormat::Rgba8Unorm,
            label: "wrong-size",
        },
    )
    .unwrap();
    let mut degenerate = empty_list();
    degenerate.items.push(shape(shape_item(
        square(),
        Affine2::scale(0.0, 0.0),
        DisplayColor::WHITE,
    )));
    let mut errors = Vec::new();
    let frame = rig.frame(&list, ABSENT);
    let renderer = &mut rig.renderer;
    let target = &rig.target;
    run_one_shot_blocking(&context, |cb| {
        target.record_prepare_for_render(cb, ColorTargetPreviousUse::NotUsedYet, ContentOnPrepare::Discard)?;
        RasterPass::begin(&context, cb, target.attachment(), AttachmentLoad::Clear(CLEAR))?.end();
        errors.push(renderer.record_frame(cb, 7, target.attachment(), &frame).unwrap_err());
        errors.push(
            renderer
                .record_frame(cb, 0, other_format.attachment(), &frame)
                .unwrap_err(),
        );
        errors.push(
            renderer
                .record_frame(cb, 0, wrong_size.attachment(), &frame)
                .unwrap_err(),
        );
        let mirrored = ui_render_vulkan::UiFrame {
            surface_transform: SurfaceTransform::HorizontalMirror,
            ..frame
        };
        errors.push(
            renderer
                .record_frame(cb, 0, target.attachment(), &mirrored)
                .unwrap_err(),
        );
        let empty_size = ui_render_vulkan::UiFrame {
            logical_size: PxSize::new(0.0, 60.0),
            ..frame
        };
        errors.push(
            renderer
                .record_frame(cb, 0, target.attachment(), &empty_size)
                .unwrap_err(),
        );
        let bad_list = ui_render_vulkan::UiFrame {
            draw_list: &degenerate,
            ..frame
        };
        errors.push(
            renderer
                .record_frame(cb, 0, target.attachment(), &bad_list)
                .unwrap_err(),
        );
        // Успешный кадр занимает слот 1; повторная запись в него до release — «слот в полёте».
        renderer.record_frame(cb, 1, target.attachment(), &frame).unwrap();
        errors.push(renderer.record_frame(cb, 1, target.attachment(), &frame).unwrap_err());
        Ok(())
    })
    .unwrap();
    assert!(
        matches!(errors[0], UiRenderError::SlotOutOfRange { slot: 7, slots: 2 }),
        "{:?}",
        errors[0]
    );
    assert!(
        matches!(errors[1], UiRenderError::TargetFormatMismatch { .. }),
        "{:?}",
        errors[1]
    );
    assert!(
        matches!(errors[2], UiRenderError::TargetSizeMismatch { .. }),
        "{:?}",
        errors[2]
    );
    assert!(
        matches!(errors[3], UiRenderError::UnsupportedSurfaceTransform(_)),
        "{:?}",
        errors[3]
    );
    assert!(
        matches!(errors[4], UiRenderError::InvalidLogicalSize { .. }),
        "{:?}",
        errors[4]
    );
    assert!(
        matches!(
            errors[5],
            UiRenderError::InvalidDrawList(DrawListError::DegenerateTransform { item: 0, .. })
        ),
        "{:?}",
        errors[5]
    );
    assert!(
        matches!(errors[6], UiRenderError::SlotInFlight { slot: 1 }),
        "{:?}",
        errors[6]
    );
    let counters = rig.renderer.counters();
    assert_eq!((counters.frames_recorded, counters.frames_failed), (1, 7));
    // Слот 0 не пострадал от неудачных попыток; слот 1 освобождается явно и снова годен.
    rig.renderer.release_slot_after_fence(1).unwrap();
    assert!(matches!(
        rig.renderer.release_slot_after_fence(9),
        Err(UiRenderError::SlotOutOfRange { .. })
    ));
    assert_no_validation_errors(&context);
}
