//! Текст и иконки: SDF-атлас читается шейдером, глифы встают куда надо, покрытие совпадает с CPU-декодом атласа.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::mirror::over;
use support::{CLEAR, Rig, assert_no_validation_errors, empty_list, gpu_context_or_skip, pixel};
use ui_draw::atlas::{AtlasRegion, sdf_atlas};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, IconItem, TextItem};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::icons::{IconId, icon};
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};
use ui_draw::transform::Affine2;
use ui_render_vulkan::BackdropInput;

const ABSENT: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor::BLACK,
};

/// Допуск покрытия против CPU-декода: сэмплер GPU интерполирует 8-битные текселы и округляет вес, CPU считает в f32;
/// расхождение — доли одного уровня байта (1/255 ≈ 0,004 покрытия на тексель/пиксель при масштабе 1:1), берём 0,03 с запасом
/// на разное округление записи в Rgba8.
const COVERAGE_TOLERANCE: f32 = 0.03;

/// Билинейная выборка SDF-атласа в точке `uv` по правилам Vulkan (центры текселей в +0,5, края зажаты); сразу в текселях расстояния.
fn atlas_distance_texels(uv: [f32; 2]) -> f32 {
    let atlas = sdf_atlas();
    let x = uv[0] * atlas.width as f32 - 0.5;
    let y = uv[1] * atlas.height as f32 - 0.5;
    let fetch = |tx: i64, ty: i64| {
        let tx = tx.clamp(0, i64::from(atlas.width) - 1) as usize;
        let ty = ty.clamp(0, i64::from(atlas.height) - 1) as usize;
        f32::from(atlas.texels[ty * atlas.width as usize + tx])
    };
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i64, y0 as i64);
    let top = fetch(ix, iy) * (1.0 - fx) + fetch(ix + 1, iy) * fx;
    let bottom = fetch(ix, iy + 1) * (1.0 - fx) + fetch(ix + 1, iy + 1) * fx;
    let byte = top * (1.0 - fy) + bottom * fy;
    (byte - 128.0) / 127.0 * atlas.spread_texels
}

/// Один размещённый глиф для CPU-рендера: рамка на экране и область атласа.
struct ScreenGlyph {
    rect: PxRect,
    region: AtlasRegion,
}

fn cpu_render(glyphs: &[ScreenGlyph], width: u32, height: u32, color: DisplayColor) -> Vec<[f32; 4]> {
    let atlas = sdf_atlas();
    let mut pixels = vec![CLEAR; (width * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let point = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
            for glyph in glyphs {
                if !(point.x >= glyph.rect.origin.x
                    && point.x < glyph.rect.max_x()
                    && point.y >= glyph.rect.origin.y
                    && point.y < glyph.rect.max_y())
                {
                    continue;
                }
                let fx = (point.x - glyph.rect.origin.x) / glyph.rect.size.width;
                let fy = (point.y - glyph.rect.origin.y) / glyph.rect.size.height;
                let uv = [
                    glyph.region.uv_min[0] + (glyph.region.uv_max[0] - glyph.region.uv_min[0]) * fx,
                    glyph.region.uv_min[1] + (glyph.region.uv_max[1] - glyph.region.uv_min[1]) * fy,
                ];
                let texels_wide = (glyph.region.uv_max[0] - glyph.region.uv_min[0]) * atlas.width as f32;
                let texels_per_px = texels_wide / glyph.rect.size.width;
                let coverage = (0.5 - atlas_distance_texels(uv) / texels_per_px).clamp(0.0, 1.0);
                let source = [color.r * coverage, color.g * coverage, color.b * coverage, coverage];
                pixels[(y * width + x) as usize] = over(pixels[(y * width + x) as usize], source);
            }
        }
    }
    pixels
}

fn max_difference(actual: &[[f32; 4]], expected: &[[f32; 4]]) -> f32 {
    actual
        .iter()
        .zip(expected)
        .flat_map(|(a, e)| (0..3).map(move |c| (a[c] - e[c]).abs()))
        .fold(0.0, f32::max)
}

#[test]
fn photo_label_is_drawn_from_the_atlas_and_matches_cpu_decoding() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    // 64 px на em при 64 текселях на em: один тексель атласа = один пиксель экрана.
    let style = TextStyle {
        face: FontFace::Semibold,
        size_px: 64.0,
        tracking_em: 0.0,
        figures: Figures::Proportional,
    };
    let run = layout_line("PHOTO", &style).unwrap();
    assert_eq!(run.glyphs.len(), 5);
    let (width, height) = (260_u32, 110_u32);
    let origin = Affine2::translate(10.0, 80.0);
    let mut list = empty_list();
    list.items.push(DrawItem::Text(TextItem {
        run: run.clone(),
        transform: origin,
        color: DisplayColor::WHITE,
        opacity: Opacity::OPAQUE,
    }));
    let mut rig = Rig::new(
        &context,
        (width, height),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!((report.glyph_instances, report.draw_calls), (5, 1));

    let glyphs: Vec<ScreenGlyph> = run
        .glyphs
        .iter()
        .map(|glyph| ScreenGlyph {
            rect: PxRect::new(
                PxPoint::new(glyph.quad.origin.x + 10.0, glyph.quad.origin.y + 80.0),
                glyph.quad.size,
            ),
            region: glyph.region,
        })
        .collect();
    let expected = cpu_render(&glyphs, width, height, DisplayColor::WHITE);
    let worst = max_difference(&pixels, &expected);
    assert!(
        worst <= COVERAGE_TOLERANCE,
        "покрытие расходится с CPU-декодом атласа на {worst}"
    );

    // Вне рамок глифов цель не тронута (ровно цвет очистки), а внутри штрихов — почти сплошной белый.
    let outside_rects = |x: u32, y: u32| {
        !glyphs.iter().any(|g| {
            (x as f32 + 0.5) >= g.rect.origin.x - 1.0
                && (x as f32 + 0.5) < g.rect.max_x() + 1.0
                && (y as f32 + 0.5) >= g.rect.origin.y - 1.0
                && (y as f32 + 0.5) < g.rect.max_y() + 1.0
        })
    };
    let mut checked_outside = 0;
    let mut stroke_centers = 0;
    for y in 0..height {
        for x in 0..width {
            let value = pixel(&pixels, width, x, y);
            if outside_rects(x, y) {
                checked_outside += 1;
                assert!(
                    (value[0] - CLEAR[0]).abs() < 0.005,
                    "вне глифов нарисовано в ({x},{y}): {value:?}"
                );
            }
            if value[0] > 0.95 {
                stroke_centers += 1;
            }
        }
    }
    assert!(checked_outside > 10_000, "проверено вне глифов: {checked_outside}");
    assert!(
        stroke_centers > 300,
        "в штрихах букв должно быть много белых пикселей, найдено {stroke_centers}"
    );
    // Центр вертикального штриха «P»: точка с наименьшим расстоянием в рамке первой буквы.
    let first = &glyphs[0];
    let mut best = (f32::MAX, (0_u32, 0_u32));
    for y in first.rect.origin.y as u32..first.rect.max_y() as u32 {
        for x in first.rect.origin.x as u32..first.rect.max_x() as u32 {
            let fx = (x as f32 + 0.5 - first.rect.origin.x) / first.rect.size.width;
            let fy = (y as f32 + 0.5 - first.rect.origin.y) / first.rect.size.height;
            let uv = [
                first.region.uv_min[0] + (first.region.uv_max[0] - first.region.uv_min[0]) * fx,
                first.region.uv_min[1] + (first.region.uv_max[1] - first.region.uv_min[1]) * fy,
            ];
            let distance = atlas_distance_texels(uv);
            if distance < best.0 {
                best = (distance, (x, y));
            }
        }
    }
    let center = pixel(&pixels, width, best.1.0, best.1.1);
    assert!(
        best.0 < -1.5 && center[0] > 0.9,
        "центр штриха «P» в {:?}: {center:?}",
        best.1
    );
    drop((rig, list));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn grid_icon_is_drawn_inside_its_square_and_matches_cpu_decoding() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (128_u32, 128_u32);
    let size = PxSize::new(64.0, 64.0);
    let center = PxPoint::new(64.0, 64.0);
    let mut list = empty_list();
    list.items.push(DrawItem::Icon(IconItem {
        icon: IconId::Grid,
        transform: Affine2::translate(center.x, center.y),
        size_px: size,
        color: DisplayColor::WHITE,
        opacity: Opacity::OPAQUE,
    }));
    let mut rig = Rig::new(
        &context,
        (width, height),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!(report.glyph_instances, 1);

    let glyph = icon(IconId::Grid);
    let quad = glyph.quad_in_square;
    let rect = PxRect::new(
        PxPoint::new(
            center.x - 32.0 + quad.origin.x * 64.0,
            center.y - 32.0 + quad.origin.y * 64.0,
        ),
        PxSize::new(quad.size.width * 64.0, quad.size.height * 64.0),
    );
    let expected = cpu_render(
        &[ScreenGlyph {
            rect,
            region: glyph.region,
        }],
        width,
        height,
        DisplayColor::WHITE,
    );
    let worst = max_difference(&pixels, &expected);
    assert!(
        worst <= COVERAGE_TOLERANCE,
        "иконка расходится с CPU-декодом на {worst}"
    );
    let bright = pixels.iter().filter(|p| p[0] > 0.9).count();
    assert!(
        bright > 200,
        "иконка «сетка» должна дать заметную белую площадь, найдено {bright}"
    );
    // Вне квадрата иконки с полем расстояний — ничего.
    assert!((pixel(&pixels, width, 2, 2)[0] - CLEAR[0]).abs() < 0.005);
    assert_no_validation_errors(&context);
}
