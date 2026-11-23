//! Замер GPU на реальном драйвере (ориентир, не провал): типичный кадр камеры на 1080 × 2412 логических px —
//! 8 стеклянных элементов, подписи и иконки. Метки времени вокруг пирамиды и прохода UI; цифры печатаются в `--nocapture`.
//! Тест проваливается только если кадр не нарисовался или метки нечитаемы при их поддержке.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use ash::vk;
use gpu_vulkan::color_target::{
    ColorTarget, ColorTargetDesc, ColorTargetFormat, ColorTargetPreviousUse, ContentOnPrepare,
};
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::raster::{AttachmentLoad, RasterPass};
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::{assert_no_validation_errors, empty_list, full_screen_placement, glass_group, gpu_context_or_skip};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, IconItem, TextItem};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};
use ui_draw::transform::Affine2;
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::{BackdropInput, FrameMarker, UiFrame, UiRenderer, UiRendererConfig};

const WIDTH: u32 = 1080;
const HEIGHT: u32 = 2412;
const PX_PER_PT: f32 = 1080.0 / 440.0;
const FRAMES: u64 = 30;

fn typical_frame() -> ui_draw::draw_list::DrawList {
    let mut list = empty_list();
    let style = GlassStyle::default();
    // Нижний ряд: 4 круглые кнопки по 48 pt и капсула затвора; сверху панель и два значка-кнопки.
    let button = 48.0 * PX_PER_PT;
    for index in 0..4 {
        let center = PxPoint::new(180.0 + index as f32 * 240.0, 2150.0);
        list.items.push(DrawItem::Glass(glass_group(
            &style,
            &[(ShapeGeometry::Circle { diameter: button }, center)],
            0.0,
        )));
    }
    list.items.push(DrawItem::Glass(glass_group(
        &style,
        &[(
            ShapeGeometry::Capsule {
                size: PxSize::new(560.0, 120.0),
            },
            PxPoint::new(540.0, 1900.0),
        )],
        0.0,
    )));
    for index in 0..3 {
        list.items.push(DrawItem::Glass(glass_group(
            &style,
            &[(
                ShapeGeometry::Circle { diameter: button },
                PxPoint::new(200.0 + index as f32 * 340.0, 200.0),
            )],
            0.0,
        )));
    }
    let label = TextStyle {
        face: FontFace::Semibold,
        size_px: 40.0,
        tracking_em: 0.0,
        figures: Figures::Tabular,
    };
    for index in 0..6 {
        list.items.push(DrawItem::Text(TextItem {
            run: layout_line("PHOTO  VIDEO  PORTRAIT 1.0x", &label).unwrap(),
            transform: Affine2::translate(60.0, 1700.0 + index as f32 * 20.0),
            color: DisplayColor::WHITE,
            opacity: Opacity::OPAQUE,
        }));
    }
    for index in 0..6 {
        list.items.push(DrawItem::Icon(IconItem {
            icon: IconId::Grid,
            transform: Affine2::translate(100.0 + index as f32 * 150.0, 400.0),
            size_px: PxSize::new(64.0, 64.0),
            color: DisplayColor::WHITE,
            opacity: Opacity::OPAQUE,
        }));
    }
    list
}

fn median(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values[values.len() / 2]
}

#[test]
fn typical_camera_frame_gpu_time_on_the_desktop_driver() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = SyntheticBackdrop::new(
        &context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::Photo,
    )
    .unwrap();
    let format = ColorTargetFormat::A2B10G10R10Unorm;
    let target = ColorTarget::new(
        &context,
        &ColorTargetDesc {
            extent: vk::Extent2D {
                width: WIDTH,
                height: HEIGHT,
            },
            format,
            label: "timing",
        },
    )
    .unwrap();
    let mut renderer = UiRenderer::new(&context, &UiRendererConfig::new(format.vk_format(), 2)).unwrap();
    let mut timestamps = GpuTimestamps::new(&context, 2, &["start", "pyramid_done", "ui_done"]).unwrap();
    let list = typical_frame();
    let placement = full_screen_placement(WIDTH as f32, HEIGHT as f32);

    let mut rebuild = (Vec::new(), Vec::new());
    let mut recompose = (Vec::new(), Vec::new());
    let mut supported = true;
    for frame_number in 0..FRAMES {
        // Чётные кадры — новый кадр фона (пирамида строится), нечётные — тот же фон (Recompose: пирамида переиспользована).
        let source = backdrop.source(frame_number / 2);
        let frame = UiFrame {
            draw_list: &list,
            logical_size: PxSize::new(WIDTH as f32, HEIGHT as f32),
            surface_transform: SurfaceTransform::Identity,
            backdrop: BackdropInput::Live {
                source: &source,
                placement,
            },
            live_images: ui_render_vulkan::LiveImages::none(),
        };
        let slot = (frame_number % 2) as usize;
        let mut result = None;
        run_one_shot_blocking(&context, |command_buffer| {
            timestamps.record_reset(command_buffer, slot)?;
            target.record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Discard,
            )?;
            RasterPass::begin(
                &context,
                command_buffer,
                target.attachment(),
                AttachmentLoad::Clear([0.0, 0.0, 0.0, 1.0]),
            )?
            .end();
            let mut marker_error = None;
            result = Some(renderer.record_frame_marked(
                command_buffer,
                slot,
                target.attachment(),
                &frame,
                &mut |marker, cb| {
                    let (point, stage) = match marker {
                        FrameMarker::Start => (0, TimestampStage::AllCommandsDone),
                        FrameMarker::PyramidDone => (1, TimestampStage::AllCommandsDone),
                        FrameMarker::UiPassDone => (2, TimestampStage::AllCommandsDone),
                    };
                    if let Err(error) = timestamps.record_point(cb, slot, point, stage) {
                        marker_error = Some(error);
                    }
                },
            ));
            marker_error.map_or(Ok(()), Err)
        })
        .unwrap();
        let report = result.unwrap().unwrap();
        renderer.release_slot_after_fence(slot).unwrap();
        match timestamps.try_collect(slot).unwrap() {
            TimestampCollect::Ready(spans) => {
                let bucket = if frame_number >= 4 {
                    if frame_number % 2 == 0 {
                        &mut rebuild
                    } else {
                        &mut recompose
                    }
                } else {
                    continue;
                };
                bucket.0.push(spans.spans[0].duration_ns);
                bucket.1.push(spans.spans[1].duration_ns);
            }
            TimestampCollect::Unsupported => supported = false,
            other => panic!("метки после блокирующей отправки обязаны быть готовы: {other:?}"),
        }
        assert_eq!(
            (report.glass_groups, report.shape_instances),
            (8, 0),
            "кадр нарисован целиком"
        );
        assert_eq!(
            report.draw_calls, 2,
            "8 стёкол подряд — один пакет, подписи и иконки подряд — второй"
        );
    }
    if supported {
        let ms = |ns: u64| ns as f64 / 1.0e6;
        println!(
            "GPU, кадр {WIDTH}x{HEIGHT}, 8 стекол, 6 подписей, 6 иконок (медиана по {} кадрам):",
            rebuild.0.len()
        );
        println!(
            "  кадр с перестройкой пирамиды: пирамида {:.3} мс + проход UI {:.3} мс",
            ms(median(rebuild.0.clone())),
            ms(median(rebuild.1.clone()))
        );
        println!(
            "  кадр Recompose (пирамида переиспользована): пирамида {:.3} мс + проход UI {:.3} мс",
            ms(median(recompose.0.clone())),
            ms(median(recompose.1.clone()))
        );
        assert!(median(rebuild.0.clone()) > 0, "пирамида заняла ненулевое время");
    } else {
        println!("очередь не поддерживает метки времени: замер пропущен");
    }
    assert_no_validation_errors(&context);
}
