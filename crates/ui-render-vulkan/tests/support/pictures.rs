//! Помощники тестов картинок UI (сессия 14g): тестовая картинка, элемент, CPU-рендер списка с картинками
//! и запись кадра без немедленного освобождения слота.

use gpu_vulkan::color_target::{ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, RasterPass};
use ui_draw::color::{DisplayPrimaries, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageItem, shade_image_pixel};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::picture::{PictureEncoding, PictureSize, UiPicture};
use ui_draw::transform::Affine2;
use ui_render_vulkan::{BackdropInput, UiFrameReport, UiRenderError};

use super::mirror::{over, shape_pixel};
use super::{CLEAR, Rig};

/// Картинка `width × height`: красный растёт по x, зелёный по y, синий — шахматка, альфа убывает по x
/// (прямая альфа: ошибка предумножения при фильтрации сразу видна). Асимметрична по обеим осям.
pub fn gradient_picture(width: u32, height: u32) -> UiPicture {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let red = (x * 255 / (width - 1)) as u8;
            let green = (y * 255 / (height - 1)) as u8;
            let blue = if (x + y) % 2 == 0 { 200 } else { 40 };
            let alpha = (255 - x * 8) as u8;
            bytes.extend_from_slice(&[red, green, blue, alpha]);
        }
    }
    UiPicture::from_rgba8(PictureSize::new(width, height), PictureEncoding::Srgb, bytes).unwrap()
}

/// Однотонная картинка.
pub fn flat_picture(width: u32, height: u32, rgba: [u8; 4]) -> UiPicture {
    let bytes = (0..width * height).flat_map(|_| rgba).collect();
    UiPicture::from_rgba8(PictureSize::new(width, height), PictureEncoding::Srgb, bytes).unwrap()
}

/// Элемент-картинка в рамке `frame`, с заданным преобразованием и прозрачностью.
pub fn image_item(picture: &UiPicture, frame: (f32, f32), transform: Affine2, opacity: f32) -> ImageItem {
    ImageItem {
        picture: picture.clone(),
        transform,
        size_px: PxSize::new(frame.0, frame.1),
        fit: ImageFit::Cover,
        clip: ImageClip::Circle,
        opacity: Opacity::new(opacity).unwrap(),
    }
}

/// CPU-рендер списка из фигур и картинок поверх `clear` по эталонам `ui-draw` (прямая ориентация).
pub fn render_with_images(
    list: &DrawList,
    width: u32,
    height: u32,
    clear: [f32; 4],
    primaries: DisplayPrimaries,
) -> Vec<[f32; 4]> {
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let point = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
            let mut color = clear;
            for item in &list.items {
                color = match item {
                    DrawItem::Shape(shape) => over(color, shape_pixel(shape, point)),
                    DrawItem::Image(image) => {
                        let shaded = shade_image_pixel(image, point, primaries, 1.0).unwrap();
                        over(color, [shaded.r, shaded.g, shaded.b, shaded.a])
                    }
                    other => panic!("CPU-рендер не умеет {other:?}"),
                };
            }
            pixels.push(color);
        }
    }
    pixels
}

/// Пишет кадр в слот `slot` поверх очищенной цели и НЕ освобождает слот (вызывающий отпустит его сам,
/// когда нужно проверить отложенное освобождение). Возвращает результат записи и пиксели цели.
pub fn record_keeping_slot(
    rig: &mut Rig,
    slot: usize,
    list: &DrawList,
) -> (Result<UiFrameReport, UiRenderError>, Vec<[f32; 4]>) {
    let backdrop = BackdropInput::Absent {
        fill: ui_draw::color::DisplayColor::BLACK,
    };
    let frame = rig.frame(list, backdrop);
    let mut outcome = None;
    let renderer = &mut rig.renderer;
    let target = &rig.target;
    let context = &rig.context;
    run_one_shot_blocking(context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        RasterPass::begin(
            context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(CLEAR),
        )?
        .end();
        outcome = Some(renderer.record_frame(command_buffer, slot, target.attachment(), &frame));
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&rig.target, ColorTargetPreviousUse::Rendered).unwrap();
    (outcome.unwrap(), pixels)
}

/// Пишет кадр в слот `slot`, но буфер НЕ отправляет (как сессия при сбое сабмита после успешной записи):
/// запись идёт в буфер разовой отправки, а замыкание возвращает ошибку — такой буфер не уходит в очередь.
/// Возвращает результат записи кадра рендером.
pub fn record_without_submit(rig: &mut Rig, slot: usize, list: &DrawList) -> Result<UiFrameReport, UiRenderError> {
    let backdrop = BackdropInput::Absent {
        fill: ui_draw::color::DisplayColor::BLACK,
    };
    let frame = rig.frame(list, backdrop);
    let mut outcome = None;
    let renderer = &mut rig.renderer;
    let target = &rig.target;
    let context = &rig.context;
    let submitted = run_one_shot_blocking(context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        outcome = Some(renderer.record_frame(command_buffer, slot, target.attachment(), &frame));
        Err(gpu_vulkan::error::GpuError::InvalidState {
            what: "тест: записанный кадр не отправляется".to_owned(),
        })
    });
    assert!(submitted.is_err(), "буфер кадра не должен был уйти в очередь");
    outcome.unwrap()
}
