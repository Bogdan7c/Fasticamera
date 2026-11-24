//! Узкая область рисования стекла (сессия 16a, G2): scissor по области вклада группы не меняет картинку.
//!
//! Два доказательства:
//! 1. РАВЕНСТВО: кадр боевого рендера (scissor по `GlassGroupItem::draw_rect_px`) совпадает бит-в-бит с кадром
//!    того же рендера, рисующего квад целиком (как до оптимизации): матрица форм × материалов (слияние, большие
//!    тень и свечение, нажатие, поворот, край экрана, прозрачность), продуктовые сцены, повороты цели.
//! 2. ДОСТАТОЧНОСТЬ: при рисовании ЦЕЛИКОМ в прозрачную цель все ненулевые пиксели лежат внутри
//!    `draw_rect_px`, отступив от его границы не меньше запаса на сглаживание (1 px). То есть границы
//!    не просто совпали на выбранных сценах: за ними группа действительно ничего не рисует.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod glass_fast_paths_cases;
mod saturation_support;
mod support;

use ash::vk;
use glass_fast_paths_cases::{OPACITY_EFFECT_COMBOS, glass_case_list, material_cases, shape_cases};
use gpu_vulkan::backdrop::BackdropSource;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::test_pattern::SurfaceTransform;
use saturation_support::{BACKDROP_EXTENT, VIEWFINDER, panel_draw_list, typical_draw_list};
use support::{CLEAR, Rig, assert_no_validation_errors, gpu_context_or_skip};
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::testing_reference::whole_glass_quad_renderer;
use ui_render_vulkan::{BackdropInput, BackdropPlacement, UiRendererConfig};

const CANVAS: (u32, u32) = (400, 300);
/// Запас квада на сглаживание, px (`ui_draw::draw_list::DRAW_RECT_EDGE_MARGIN_PX`).
const EDGE_MARGIN_PX: f32 = 1.0;
/// Прозрачная очистка цели: после неё ненулевой пиксель — это вклад группы.
const TRANSPARENT: [f32; 4] = [0.0; 4];

/// Рендер с scissor и тот же рендер с полным квадом.
struct Pair {
    tight: Rig,
    whole: Rig,
}

impl Pair {
    fn new(context: &GpuContext, logical: (u32, u32), transform: SurfaceTransform, format: ColorTargetFormat) -> Self {
        let tight = Rig::new(context, logical, transform, format);
        let mut whole = Rig::new(context, logical, transform, format);
        whole.renderer = whole_glass_quad_renderer(context, &UiRendererConfig::new(format.vk_format(), 2)).unwrap();
        Self { tight, whole }
    }
}

fn assert_bit_identical(label: &str, width: u32, tight: &[[f32; 4]], whole: &[[f32; 4]]) {
    assert_eq!(tight.len(), whole.len(), "{label}: разный размер");
    let mut total = 0_usize;
    let mut first = Vec::new();
    for (index, (a, b)) in tight.iter().zip(whole).enumerate() {
        if a.iter().zip(b).any(|(x, y)| x.to_bits() != y.to_bits()) {
            total += 1;
            if first.len() < 4 {
                first.push(format!(
                    "({},{}): узкий {a:?} против полного {b:?}",
                    index as u32 % width,
                    index as u32 / width
                ));
            }
        }
    }
    assert!(
        total == 0,
        "{label}: scissor изменил {total} пикселей; первые: {}",
        first.join("; ")
    );
}

struct Backdrop {
    _image: SyntheticBackdrop,
    source: BackdropSource,
}

impl Backdrop {
    fn new(context: &GpuContext, extent: vk::Extent2D) -> Self {
        let image = SyntheticBackdrop::new(context, extent, BackdropPattern::Photo).unwrap();
        let source = image.source(1);
        Self { _image: image, source }
    }

    fn live(&self, destination: PxRect) -> BackdropInput<'_> {
        BackdropInput::Live {
            source: &self.source,
            placement: BackdropPlacement { destination },
        }
    }
}

fn canvas_rect() -> PxRect {
    PxRect::new(PxPoint::new(0.0, 0.0), PxSize::new(CANVAS.0 as f32, CANVAS.1 as f32))
}

fn absent() -> BackdropInput<'static> {
    BackdropInput::Absent {
        fill: DisplayColor::rgb(0.25, 0.4, 0.55),
    }
}

fn first_group(list: &DrawList) -> &GlassGroupItem {
    list.items
        .iter()
        .find_map(|item| match item {
            DrawItem::Glass(group) => Some(group),
            _ => None,
        })
        .expect("в сцене есть стеклянная группа")
}

/// Ненулевые пиксели лежат внутри `rect`, отступив от его границы на `EDGE_MARGIN_PX`. Возвращает число
/// ненулевых пикселей (чтобы вызывающий проверил, что сцена не пуста).
fn assert_contribution_inside(label: &str, pixels: &[[f32; 4]], width: u32, rect: &PxRect) -> usize {
    let mut nonzero = 0_usize;
    for (index, pixel) in pixels.iter().enumerate() {
        if pixel.iter().all(|channel| *channel == 0.0) {
            continue;
        }
        nonzero += 1;
        let center = PxPoint::new((index as u32 % width) as f32 + 0.5, (index as u32 / width) as f32 + 0.5);
        let inside = center.x >= rect.origin.x + EDGE_MARGIN_PX
            && center.x <= rect.max_x() - EDGE_MARGIN_PX
            && center.y >= rect.origin.y + EDGE_MARGIN_PX
            && center.y <= rect.max_y() - EDGE_MARGIN_PX;
        assert!(
            inside,
            "{label}: ненулевой пиксель {pixel:?} в {center:?} вне области вклада {rect:?} (с запасом {EDGE_MARGIN_PX} px)"
        );
    }
    nonzero
}

/// Равенство и достаточность на всей матрице форм × материалов; сочетание прозрачности и материализации
/// пробегает по кругу, фон — живой и пустой попеременно.
#[test]
fn tight_scissor_matches_whole_quad_and_contains_every_contribution() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = Backdrop::new(
        &context,
        vk::Extent2D {
            width: CANVAS.0,
            height: CANVAS.1,
        },
    );
    let mut pair = Pair::new(
        &context,
        CANVAS,
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let (mut scenes, mut visible, mut cursor) = (0_usize, 0_usize, 0_usize);
    for shape in shape_cases() {
        for material in material_cases() {
            let combo = OPACITY_EFFECT_COMBOS[cursor % OPACITY_EFFECT_COMBOS.len()];
            cursor += 1;
            let list = glass_case_list(&shape, &material, combo);
            let live = scenes % 3 != 2;
            let input = if live { backdrop.live(canvas_rect()) } else { absent() };
            let label = format!("{} / {} / {combo:?} / live {live}", shape.name, material.name);

            let (_, tight_pixels) = pair.tight.render(&list, input);
            let (_, whole_pixels) = pair.whole.render(&list, input);
            assert_bit_identical(&label, CANVAS.0, &tight_pixels, &whole_pixels);

            // Достаточность: полный квад на прозрачной цели — все вклады внутри области рисования.
            let (result, transparent_pixels) = pair.whole.render_with_clear(&list, input, TRANSPARENT);
            result.unwrap();
            let rect = first_group(&list).draw_rect_px().unwrap();
            let nonzero = assert_contribution_inside(&label, &transparent_pixels, CANVAS.0, &rect);
            scenes += 1;
            if nonzero > 500 {
                visible += 1;
            }
        }
    }
    assert!(scenes > 200, "матрица мала: {scenes}");
    assert!(
        visible * 10 >= scenes * 8,
        "вклад виден лишь в {visible} из {scenes} сцен"
    );
    println!("узкий scissor: {scenes} сцен совпали бит-в-бит, все вклады внутри области рисования");
    assert_no_validation_errors(&context);
}

/// Полный перекрёст сочетаний прозрачности и материализации на ключевых формах и материалах с сильной тенью,
/// слиянием и нажатием (где область вклада больше всего).
#[test]
fn opacity_cross_with_big_shadow_merge_and_press_is_contained() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = Backdrop::new(
        &context,
        vk::Extent2D {
            width: CANVAS.0,
            height: CANVAS.1,
        },
    );
    let mut pair = Pair::new(
        &context,
        CANVAS,
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let shapes = shape_cases();
    let materials = material_cases();
    let wanted_materials = [
        "тень большая и сильная",
        "тень с радиусом 0 (резкая)",
        "нажатие 0,9, касание далеко снаружи",
        "нажатие 0,9, касание внутри",
        "панель продукта (блюр 8 pt, тон, затемнение)",
    ];
    let mut checked = 0_usize;
    for material in materials.iter().filter(|m| wanted_materials.contains(&m.name)) {
        for shape_index in [0_usize, 3, 4, 6, 7] {
            for combo in OPACITY_EFFECT_COMBOS {
                let list = glass_case_list(&shapes[shape_index], material, combo);
                let label = format!("{} / {} / {combo:?}", shapes[shape_index].name, material.name);
                let input = backdrop.live(canvas_rect());
                let (_, tight_pixels) = pair.tight.render(&list, input);
                let (_, whole_pixels) = pair.whole.render(&list, input);
                assert_bit_identical(&label, CANVAS.0, &tight_pixels, &whole_pixels);
                let (result, transparent_pixels) = pair.whole.render_with_clear(&list, input, TRANSPARENT);
                result.unwrap();
                let rect = first_group(&list).draw_rect_px().unwrap();
                assert_contribution_inside(&label, &transparent_pixels, CANVAS.0, &rect);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, wanted_materials.len() * 5 * OPACITY_EFFECT_COMBOS.len());
    assert_no_validation_errors(&context);
}

/// Все четыре поворота цели: scissor переводится в физические координаты так же, как вершины.
#[test]
fn rotated_targets_keep_the_image_identical() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = Backdrop::new(
        &context,
        vk::Extent2D {
            width: CANVAS.0,
            height: CANVAS.1,
        },
    );
    let shapes = shape_cases();
    let materials = material_cases();
    for transform in [
        SurfaceTransform::Identity,
        SurfaceTransform::Rotate90,
        SurfaceTransform::Rotate180,
        SurfaceTransform::Rotate270,
    ] {
        let mut pair = Pair::new(&context, CANVAS, transform, ColorTargetFormat::Rgba16Float);
        for (shape_index, material_index) in [(0_usize, 0_usize), (3, 17), (7, 1)] {
            let list = glass_case_list(&shapes[shape_index], &materials[material_index], (1.0, 1.0));
            let input = backdrop.live(canvas_rect());
            let (_, tight_pixels) = pair.tight.render(&list, input);
            let (_, whole_pixels) = pair.whole.render(&list, input);
            let label = format!(
                "{transform:?} / {} / {}",
                shapes[shape_index].name, materials[material_index].name
            );
            assert!(
                tight_pixels.iter().filter(|p| **p != CLEAR).count() > 1000,
                "{label}: пусто"
            );
            assert_bit_identical(&label, pair.tight.width(), &tight_pixels, &whole_pixels);
        }
    }
    assert_no_validation_errors(&context);
}

/// Продуктовые сцены 1080 × 2412 (панель на пол-экрана и типичный кадр) в fp16 и 10 бит.
#[test]
fn product_scenes_keep_the_image_identical() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = Backdrop::new(&context, BACKDROP_EXTENT);
    let viewfinder = PxRect::new(
        PxPoint::new(VIEWFINDER.x as f32, VIEWFINDER.y as f32),
        PxSize::new(VIEWFINDER.width as f32, VIEWFINDER.height as f32),
    );
    for (name, list) in [("панель", panel_draw_list()), ("типичный кадр", typical_draw_list())] {
        for format in [ColorTargetFormat::Rgba16Float, ColorTargetFormat::A2B10G10R10Unorm] {
            let mut pair = Pair::new(&context, (1080, 2412), SurfaceTransform::Identity, format);
            let input = backdrop.live(viewfinder);
            let (_, tight_pixels) = pair.tight.render(&list, input);
            let (_, whole_pixels) = pair.whole.render(&list, input);
            let label = format!("{name} / {format:?}");
            assert!(
                tight_pixels.iter().filter(|p| **p != CLEAR).count() > 50_000,
                "{label}: пусто"
            );
            assert_bit_identical(&label, 1080, &tight_pixels, &whole_pixels);
            // Достаточность на продуктовой сцене (все группы сразу): ненулевой вклад только внутри объединения
            // областей рисования групп.
            let (result, transparent) = pair.whole.render_with_clear(&list, input, TRANSPARENT);
            result.unwrap();
            let rects: Vec<PxRect> = list
                .items
                .iter()
                .filter_map(|item| match item {
                    DrawItem::Glass(group) => group.draw_rect_px(),
                    _ => None,
                })
                .collect();
            for (index, pixel) in transparent.iter().enumerate() {
                if pixel.iter().all(|channel| *channel == 0.0) {
                    continue;
                }
                let center = PxPoint::new((index % 1080) as f32 + 0.5, (index / 1080) as f32 + 0.5);
                let inside_some = rects.iter().any(|rect| {
                    center.x >= rect.origin.x + EDGE_MARGIN_PX
                        && center.x <= rect.max_x() - EDGE_MARGIN_PX
                        && center.y >= rect.origin.y + EDGE_MARGIN_PX
                        && center.y <= rect.max_y() - EDGE_MARGIN_PX
                });
                // Текст и иконки типичного кадра рисуются не стеклом: их вклад в этой проверке не учитывается,
                // поэтому сравниваем только в панели (там кроме стекла ничего нет).
                if name == "панель" {
                    assert!(
                        inside_some,
                        "{label}: вклад {pixel:?} в {center:?} вне областей рисования"
                    );
                }
            }
        }
    }
    assert_no_validation_errors(&context);
}
