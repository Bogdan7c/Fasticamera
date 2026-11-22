//! Golden-сцены: полный кадр UI (фигуры, текст, иконки, стекло над синтетическим «фото») сверяется с эталонами в
//! `test-assets/ui-render-vulkan/golden/*.pam`. Генерация: `FASTICAMERA_UPDATE_GOLDEN=1 cargo test -p ui-render-vulkan --test golden`;
//! PNG для глаз: `FASTICAMERA_GOLDEN_PREVIEW_DIR=<каталог>` (вместе с генерацией или при обычной сверке).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use std::path::{Path, PathBuf};

use ash::vk;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::image::{decode_pam, encode_pam, encode_png};
use support::pictures::{gradient_picture, image_item};
use support::{
    Rig, assert_no_validation_errors, empty_list, full_screen_placement, glass_group, gpu_context_or_skip, shape,
    shape_item,
};
use ui_draw::color::{DisplayColor, DisplayPrimaries, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, IconItem, StrokeStyle, TextItem};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::{GlassStyle, ShadowParamsPx};
use ui_draw::icons::IconId;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};
use ui_draw::transform::Affine2;
use ui_render_vulkan::BackdropInput;
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};

const WIDTH: u32 = 360;
const HEIGHT: u32 = 240;
/// Допуск golden на канал: 2/1023 — два шага 10-битного канала цели A2B10G10R10. Тот же драйвер и те же шейдеры дают
/// побитно одинаковый результат; два шага оставлены на пересборки Mesa с другим порядком FMA/округления в smoothstep.
/// Заметная ошибка рисунка (сдвиг на пиксель, смена формулы) даёт на порядок больше.
const GOLDEN_TOLERANCE: f32 = 2.0 / 1023.0;
/// 1,5 px на pt: кнопка 44 pt ≈ 66 px помещается в сцену 360 × 240.
const PX_PER_PT: f32 = 1.5;
const UPDATE_ENV: &str = "FASTICAMERA_UPDATE_GOLDEN";
const PREVIEW_ENV: &str = "FASTICAMERA_GOLDEN_PREVIEW_DIR";

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-assets/ui-render-vulkan/golden")
}

/// Записывает или сверяет эталон `name`.
fn check_golden(name: &str, pixels: &[[f32; 4]]) {
    if let Ok(preview) = std::env::var(PREVIEW_ENV) {
        std::fs::create_dir_all(&preview).unwrap();
        std::fs::write(
            Path::new(&preview).join(format!("{name}.png")),
            encode_png(WIDTH, HEIGHT, pixels),
        )
        .unwrap();
    }
    let path = golden_dir().join(format!("{name}.pam"));
    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&path, encode_pam(WIDTH, HEIGHT, pixels)).unwrap();
        eprintln!("эталон записан: {}", path.display());
        return;
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "нет эталона {} ({error}). Сгенерируйте: {UPDATE_ENV}=1 cargo test -p ui-render-vulkan --test golden",
            path.display()
        )
    });
    let (width, height, expected) =
        decode_pam(&bytes).unwrap_or_else(|error| panic!("эталон {} повреждён: {error}", path.display()));
    assert_eq!((width, height), (WIDTH, HEIGHT), "размер эталона {name}");
    let mut worst = (0.0_f32, 0_usize);
    for (index, (actual, wanted)) in pixels.iter().zip(&expected).enumerate() {
        // Альфа A2B10G10R10 — 2 бита и в сцене всегда 1; сравниваем цвет.
        for channel in 0..3 {
            let difference = (actual[channel] - wanted[channel]).abs();
            if difference > worst.0 {
                worst = (difference, index);
            }
        }
    }
    assert!(
        worst.0 <= GOLDEN_TOLERANCE,
        "{name}: расхождение {} (допуск {GOLDEN_TOLERANCE}) в пикселе ({}, {}). Если изменение намеренное: {UPDATE_ENV}=1 cargo test -p ui-render-vulkan --test golden",
        worst.0,
        worst.1 as u32 % WIDTH,
        worst.1 as u32 / WIDTH
    );
}

enum Background {
    Photo,
    Absent(DisplayColor),
}

fn render_scene(name: &str, list: &DrawList, background: Background) {
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
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::A2B10G10R10Unorm,
    );
    let input = match background {
        Background::Photo => BackdropInput::Live {
            source: &source,
            placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
        },
        Background::Absent(fill) => BackdropInput::Absent { fill },
    };
    // Для сцен без живого фона цель заливается цветом Absent.
    let clear = match background {
        Background::Photo => [0.0, 0.0, 0.0, 1.0],
        Background::Absent(fill) => [fill.r, fill.g, fill.b, 1.0],
    };
    let pixels = match background {
        // Боевой порядок: композитор кладёт «фото» в цель, поверх рисуется UI.
        Background::Photo => rig.render_over_composited_backdrop(list, &source),
        Background::Absent(_) => {
            let (result, pixels) = rig.render_with_clear(list, input, clear);
            result.unwrap();
            pixels
        }
    };
    check_golden(name, &pixels);
    drop((rig, source, backdrop));
    assert_no_validation_errors(&context);
}

/// Кнопка-круг и капсула со стилем кнопки по умолчанию.
fn button_pair(list: &mut DrawList, pressed: bool) {
    let style = GlassStyle::default();
    let circle = ShapeGeometry::Circle { diameter: 66.0 };
    let capsule = ShapeGeometry::Capsule {
        size: PxSize::new(150.0, 48.0),
    };
    let mut circle_group = glass_group(&style, &[(circle, PxPoint::new(70.0, 80.0))], 0.0);
    circle_group.params = style.resolve(PxSize::new(66.0, 66.0), PX_PER_PT);
    let mut capsule_group = glass_group(&style, &[(capsule, PxPoint::new(235.0, 80.0))], 0.0);
    capsule_group.params = style.resolve(PxSize::new(150.0, 48.0), PX_PER_PT);
    if pressed {
        for group in [&mut circle_group, &mut capsule_group] {
            let scale = group.params.press.scale_for(1.0);
            let center = group.members[0].transform.apply_point(PxPoint::ORIGIN);
            group.members[0].transform = Affine2::uniform_scale(scale).then(Affine2::translate(center.x, center.y));
            group.members[0].press.amount = 1.0;
            group.members[0].press.touch_point_local = PxPoint::new(-8.0, 6.0);
        }
    }
    list.items.push(DrawItem::Glass(circle_group));
    list.items.push(DrawItem::Glass(capsule_group));
}

#[test]
fn golden_shapes() {
    let mut list = empty_list();
    let mut rect = shape_item(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(120.0, 80.0),
            corner: CornerSpec::continuous(28.0),
        },
        Affine2::translate(80.0, 70.0),
        DisplayColor::rgb(0.85, 0.25, 0.25),
    );
    rect.stroke = Some(StrokeStyle {
        width_px: 4.0,
        color: DisplayColor::WHITE,
    });
    list.items.push(shape(rect));
    let mut circle = shape_item(
        ShapeGeometry::Circle { diameter: 70.0 },
        Affine2::translate(215.0, 70.0),
        DisplayColor::new(0.2, 0.4, 0.9, 0.8),
    );
    circle.shadow = Some(ShadowParamsPx {
        radius_px: 10.0,
        alpha: 0.5,
        offset_px: 5.0,
    });
    list.items.push(shape(circle));
    list.items.push(shape(shape_item(
        ShapeGeometry::Capsule {
            size: PxSize::new(150.0, 40.0),
        },
        Affine2::rotate(-0.25).then(Affine2::translate(110.0, 180.0)),
        DisplayColor::rgb(0.2, 0.7, 0.4),
    )));
    list.items.push(shape(shape_item(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(80.0, 80.0),
            corner: CornerSpec::circular(24.0),
        },
        Affine2::translate(285.0, 175.0),
        DisplayColor::rgb(0.95, 0.75, 0.2),
    )));
    render_scene("shapes", &list, Background::Absent(DisplayColor::rgb(0.13, 0.13, 0.17)));
}

#[test]
fn golden_text_and_icons() {
    let mut list = empty_list();
    let big = TextStyle {
        face: FontFace::Semibold,
        size_px: 48.0,
        tracking_em: 0.02,
        figures: Figures::Proportional,
    };
    list.items.push(DrawItem::Text(TextItem {
        run: layout_line("PHOTO", &big).unwrap(),
        transform: Affine2::translate(24.0, 70.0),
        color: DisplayColor::WHITE,
        opacity: Opacity::OPAQUE,
    }));
    let small = TextStyle {
        face: FontFace::Regular,
        size_px: 22.0,
        tracking_em: 0.0,
        figures: Figures::Tabular,
    };
    list.items.push(DrawItem::Text(TextItem {
        run: layout_line("EXPOSURE 0.0", &small).unwrap(),
        transform: Affine2::translate(24.0, 110.0),
        color: DisplayColor::rgb(1.0, 0.8, 0.3),
        opacity: Opacity::OPAQUE,
    }));
    for (index, icon) in [IconId::Grid, IconId::Exposure].into_iter().enumerate() {
        list.items.push(DrawItem::Icon(IconItem {
            icon,
            transform: Affine2::translate(60.0 + index as f32 * 90.0, 180.0),
            size_px: PxSize::new(56.0, 56.0),
            color: DisplayColor::WHITE,
            opacity: Opacity::OPAQUE,
        }));
    }
    render_scene(
        "text_icons",
        &list,
        Background::Absent(DisplayColor::rgb(0.1, 0.12, 0.2)),
    );
}

#[test]
fn golden_glass_buttons_over_photo() {
    let mut list = empty_list();
    button_pair(&mut list, false);
    render_scene("glass_buttons", &list, Background::Photo);
}

#[test]
fn golden_glass_pressed() {
    let mut list = empty_list();
    button_pair(&mut list, true);
    render_scene("glass_pressed", &list, Background::Photo);
}

#[test]
fn golden_glass_merged_group() {
    let mut list = empty_list();
    let style = GlassStyle::default();
    let members = [
        (ShapeGeometry::Circle { diameter: 60.0 }, PxPoint::new(95.0, 120.0)),
        (
            ShapeGeometry::Capsule {
                size: PxSize::new(100.0, 44.0),
            },
            PxPoint::new(180.0, 120.0),
        ),
        (ShapeGeometry::Circle { diameter: 60.0 }, PxPoint::new(265.0, 120.0)),
    ];
    let mut group: GlassGroupItem = glass_group(&style, &members, 28.0);
    group.params = style.resolve(PxSize::new(60.0, 60.0), PX_PER_PT);
    list.items.push(DrawItem::Glass(group));
    render_scene("glass_merged", &list, Background::Photo);
}

#[test]
fn golden_glass_over_black() {
    let mut list = empty_list();
    button_pair(&mut list, false);
    render_scene("glass_absent", &list, Background::Absent(DisplayColor::BLACK));
}

/// Картинки: две миниатюры (перекрёстное затухание старой и новой) над цветной капсулой, экран Display P3.
/// Крупная неквадратная картинка обрезана кругом (Cover), вторая повёрнута на 90° и полупрозрачна.
#[test]
fn golden_image() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::with_config(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::A2B10G10R10Unorm,
        |config| config.display_primaries = DisplayPrimaries::DisplayP3,
    );
    let wide = gradient_picture(24, 16);
    let tall = gradient_picture(10, 20);
    let mut list = empty_list();
    list.items.push(shape(shape_item(
        ShapeGeometry::Capsule {
            size: PxSize::new(300.0, 50.0),
        },
        Affine2::translate(180.0, 120.0),
        DisplayColor::rgb(0.85, 0.25, 0.2),
    )));
    list.items.push(DrawItem::Image(image_item(
        &wide,
        (150.0, 110.0),
        Affine2::translate(120.0, 120.0),
        1.0,
    )));
    list.items.push(DrawItem::Image(image_item(
        &tall,
        (110.0, 150.0),
        Affine2::rotate(std::f32::consts::FRAC_PI_2).then(Affine2::translate(230.0, 120.0)),
        0.6,
    )));
    let backdrop = BackdropInput::Absent {
        fill: DisplayColor::rgb(0.2, 0.2, 0.25),
    };
    let (result, pixels) = rig.render_with_clear(&list, backdrop, [0.2, 0.2, 0.25, 1.0]);
    result.unwrap();
    check_golden("image", &pixels);
    drop((rig, list, wide, tall));
    assert_no_validation_errors(&context);
}
