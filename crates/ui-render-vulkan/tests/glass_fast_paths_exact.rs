//! Быстрые пути шейдеров стекла и фигур (сессия 16a) не меняют картинку ни на один бит.
//!
//! Простыми словами: боевые `ui_glass.frag` и `ui_shape.frag` пропускают работу, результат которой всё равно
//! умножился бы на точный ноль (одна выборка фона вместо трёх, без второго поля формы для тени под непрозрачным
//! стеклом, без `pow` вне ободка и т. д.). Здесь одну и ту же сцену рисуют два рендера: боевой и «замороженный»
//! на копиях шейдеров до оптимизаций (`shaders/reference_*`). Пиксели читаются обратно и сравниваются на ТОЧНОЕ
//! равенство (биты, не допуск). Любое расхождение — быстрый путь изменил картинку, и его надо сужать или убирать.
//!
//! Оговорка честно: цель теста — fp16 (боевой формат ступеней) или 10-бит, поэтому разница f32 меньше половины
//! шага формата в readback не видна; для «того же самого кадра» на экране это и есть критерий (экран тоже
//! квантует). Чувствительность теста проверена намеренными ошибками в шейдере (см. отчёт сессии).

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
use glass_fast_paths_cases::{
    OPACITY_EFFECT_COMBOS, glass_case_list, material_cases, opacity, rounded, shape_cases, shape_grid_lists,
};
use gpu_vulkan::backdrop::BackdropSource;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::compositor::LogicalRect;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::test_pattern::SurfaceTransform;
use saturation_support::{BACKDROP_EXTENT, VIEWFINDER, panel_draw_list, typical_draw_list};
use support::{CLEAR, Rig, assert_no_validation_errors, empty_list, gpu_context_or_skip, shape_item};
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::glass::{GlassStyle, ShadowParamsPx};
use ui_draw::transform::Affine2;
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::testing_reference::frozen_reference_renderer;
use ui_render_vulkan::{BackdropInput, BackdropPlacement, UiRendererConfig};

/// Холст матрицы сцен.
const CANVAS: (u32, u32) = (400, 300);

/// Два рендера на одной цели: боевой и замороженный эталон.
struct Duel {
    product: Rig,
    reference: Rig,
}

impl Duel {
    fn new(context: &GpuContext, logical: (u32, u32), format: ColorTargetFormat) -> Self {
        let product = Rig::new(context, logical, SurfaceTransform::Identity, format);
        let mut reference = Rig::new(context, logical, SurfaceTransform::Identity, format);
        // Тот же формат цели и число слотов (2), что у `Rig::new`; отличаются только шейдеры стекла и фигур.
        reference.renderer = frozen_reference_renderer(context, &UiRendererConfig::new(format.vk_format(), 2)).unwrap();
        Self { product, reference }
    }

    /// Рисует список обоими рендерами; возвращает (боевой, эталон).
    fn render(&mut self, list: &DrawList, backdrop: BackdropInput<'_>) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
        let (_, product) = self.product.render(list, backdrop);
        let (_, reference) = self.reference.render(list, backdrop);
        (product, reference)
    }
}

/// Сколько пикселей отличается от цвета очистки (нужно, чтобы сцена не была пустой и тест не стал тавтологией).
fn painted_pixels(pixels: &[[f32; 4]]) -> usize {
    pixels.iter().filter(|pixel| **pixel != CLEAR).count()
}

/// Точное (битовое) равенство кадров; при расхождении паника с адресом первых отличий.
fn assert_bit_identical(label: &str, width: u32, product: &[[f32; 4]], reference: &[[f32; 4]]) {
    assert_eq!(product.len(), reference.len(), "{label}: разный размер кадров");
    let mut mismatches = Vec::new();
    let mut total = 0_usize;
    for (index, (left, right)) in product.iter().zip(reference).enumerate() {
        let equal = left.iter().zip(right).all(|(a, b)| a.to_bits() == b.to_bits());
        if !equal {
            total += 1;
            if mismatches.len() < 4 {
                let (x, y) = (index as u32 % width, index as u32 / width);
                mismatches.push(format!("({x},{y}): боевой {left:?} против эталона {right:?}"));
            }
        }
    }
    assert!(
        total == 0,
        "{label}: быстрые пути изменили {total} пикселей из {}; первые: {}",
        product.len(),
        mismatches.join("; ")
    );
}

/// Синтетический фон «Photo» (градиент, шахматка, полосы: цвет зависит от точки выборки, значит смещение
/// выборки видно в пикселях) и источник к нему.
struct Backdrops {
    _image: SyntheticBackdrop,
    source: BackdropSource,
}

impl Backdrops {
    fn new(context: &GpuContext, extent: vk::Extent2D) -> Self {
        let image = SyntheticBackdrop::new(context, extent, BackdropPattern::Photo).unwrap();
        let source = image.source(1);
        Self { _image: image, source }
    }

    fn live(&self, placement: PxRect) -> BackdropInput<'_> {
        BackdropInput::Live {
            source: &self.source,
            placement: BackdropPlacement { destination: placement },
        }
    }
}

fn absent_backdrop() -> BackdropInput<'static> {
    BackdropInput::Absent {
        fill: DisplayColor::rgb(0.25, 0.4, 0.55),
    }
}

fn full_canvas() -> PxRect {
    PxRect::new(PxPoint::new(0.0, 0.0), PxSize::new(CANVAS.0 as f32, CANVAS.1 as f32))
}

/// Главная матрица: каждая форма × каждый материал, сочетание (прозрачность, материализация) пробегает по кругу;
/// фон — «Photo» (живой) и сплошной (Absent) попеременно.
#[test]
fn glass_matrix_is_bit_identical_to_frozen_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrops = Backdrops::new(
        &context,
        vk::Extent2D {
            width: CANVAS.0,
            height: CANVAS.1,
        },
    );
    let mut duel = Duel::new(&context, CANVAS, ColorTargetFormat::Rgba16Float);
    let (mut scenes, mut visible_scenes, mut combo_cursor) = (0_usize, 0_usize, 0_usize);
    for shape in shape_cases() {
        for material in material_cases() {
            let combo = OPACITY_EFFECT_COMBOS[combo_cursor % OPACITY_EFFECT_COMBOS.len()];
            combo_cursor += 1;
            let list = glass_case_list(&shape, &material, combo);
            let live = scenes % 3 != 2;
            let backdrop = if live {
                backdrops.live(full_canvas())
            } else {
                absent_backdrop()
            };
            let (product, reference) = duel.render(&list, backdrop);
            let label = format!(
                "{} / {} / opacity {} effect {} / {}",
                shape.name,
                material.name,
                combo.0,
                combo.1,
                if live { "Live" } else { "Absent" }
            );
            assert_bit_identical(&label, CANVAS.0, &product, &reference);
            scenes += 1;
            if painted_pixels(&reference) > 500 {
                visible_scenes += 1;
            }
        }
    }
    // Тест не должен стать тавтологией: почти все сцены (кроме effect 0) реально рисуют стекло.
    assert!(scenes > 200, "матрица слишком мала: {scenes}");
    assert!(
        visible_scenes * 10 >= scenes * 8,
        "стекло видно лишь в {visible_scenes} из {scenes} сцен"
    );
    println!("матрица стекла: {scenes} сцен, видимое стекло в {visible_scenes}; все кадры совпали бит-в-бит");
    assert_no_validation_errors(&context);
}

/// Полная перекрёстная сверка сочетаний (прозрачность × материализация) на трёх ключевых материалах и трёх формах,
/// с живым и пустым фоном: именно здесь тень, рампа и ранний выход `layer_alpha <= 0` взаимодействуют сильнее всего.
#[test]
fn opacity_and_materialization_cross_is_bit_identical() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrops = Backdrops::new(
        &context,
        vk::Extent2D {
            width: CANVAS.0,
            height: CANVAS.1,
        },
    );
    let mut duel = Duel::new(&context, CANVAS, ColorTargetFormat::Rgba16Float);
    let materials = material_cases();
    let shapes = shape_cases();
    let mut scenes = 0_usize;
    for material_index in [0_usize, 1, 2] {
        for shape_index in [0_usize, 3, 6] {
            for combo in OPACITY_EFFECT_COMBOS {
                for live in [true, false] {
                    let list = glass_case_list(&shapes[shape_index], &materials[material_index], combo);
                    let backdrop = if live {
                        backdrops.live(full_canvas())
                    } else {
                        absent_backdrop()
                    };
                    let (product, reference) = duel.render(&list, backdrop);
                    let label = format!(
                        "{} / {} / opacity {} effect {} / live {live}",
                        shapes[shape_index].name, materials[material_index].name, combo.0, combo.1
                    );
                    assert_bit_identical(&label, CANVAS.0, &product, &reference);
                    scenes += 1;
                }
            }
        }
    }
    println!("перекрёстная сверка: {scenes} сцен, все совпали бит-в-бит");
    assert_no_validation_errors(&context);
}

/// Несколько групп в одном кадре и повёрнутая цель (как на телефоне в альбомной ориентации).
#[test]
fn rotated_surface_and_multiple_groups_are_bit_identical() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrops = Backdrops::new(
        &context,
        vk::Extent2D {
            width: CANVAS.0,
            height: CANVAS.1,
        },
    );
    let shapes = shape_cases();
    let materials = material_cases();
    let mut list = empty_list();
    for (index, (shape_index, material_index)) in [(0_usize, 1_usize), (3, 0), (5, 14)].into_iter().enumerate() {
        let mut group = (shapes[shape_index].build)(&GlassStyle::default());
        (materials[material_index].apply)(&mut group);
        // Разводим группы по холсту сдвигом членов, чтобы они частично перекрывали друг друга.
        for member in &mut group.members {
            member.transform = member
                .transform
                .then(Affine2::translate(index as f32 * 17.0, index as f32 * 40.0 - 40.0));
        }
        group.opacity = opacity(if index == 1 { 0.7 } else { 1.0 });
        list.items.push(DrawItem::Glass(group));
    }
    for transform in [
        SurfaceTransform::Identity,
        SurfaceTransform::Rotate90,
        SurfaceTransform::Rotate180,
        SurfaceTransform::Rotate270,
    ] {
        let mut product = Rig::new(&context, CANVAS, transform, ColorTargetFormat::Rgba16Float);
        let mut reference = Rig::new(&context, CANVAS, transform, ColorTargetFormat::Rgba16Float);
        reference.renderer = frozen_reference_renderer(
            &context,
            &UiRendererConfig::new(ColorTargetFormat::Rgba16Float.vk_format(), 2),
        )
        .unwrap();
        let (_, product_pixels) = product.render(&list, backdrops.live(full_canvas()));
        let (_, reference_pixels) = reference.render(&list, backdrops.live(full_canvas()));
        assert!(painted_pixels(&reference_pixels) > 1000, "{transform:?}: сцена пуста");
        assert_bit_identical(
            &format!("поворот цели {transform:?}"),
            product.width(),
            &product_pixels,
            &reference_pixels,
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn shapes_are_bit_identical_to_frozen_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut duel = Duel::new(&context, CANVAS, ColorTargetFormat::Rgba16Float);
    let lists = shape_grid_lists();
    let mut shadowed_visible = 0_usize;
    for (index, list) in lists.iter().enumerate() {
        let (product, reference) = duel.render(list, absent_backdrop());
        assert_bit_identical(&format!("сетка фигур {index}"), CANVAS.0, &product, &reference);
        if painted_pixels(&reference) > 500 {
            shadowed_visible += 1;
        }
    }
    assert!(shadowed_visible * 10 >= lists.len() * 8, "сетки фигур почти пусты");
    // Одиночная фигура на непрозрачной заливке: быстрый путь «тень под непрозрачной заливкой» обязан сработать
    // и не изменить кадр (тень видна вокруг, но не внутри заливки).
    let mut list = empty_list();
    let mut item = shape_item(
        rounded(160.0, 100.0, 30.0),
        Affine2::translate(200.0, 150.0),
        DisplayColor::rgb(0.9, 0.7, 0.2),
    );
    item.shadow = Some(ShadowParamsPx {
        radius_px: 16.0,
        alpha: 0.7,
        offset_px: 10.0,
    });
    list.items.push(DrawItem::Shape(item));
    let (product, reference) = duel.render(&list, absent_backdrop());
    assert_bit_identical("непрозрачная фигура с тенью", CANVAS.0, &product, &reference);
    assert!(painted_pixels(&reference) > 10_000);
    println!("фигуры: {} сеток + одиночная; все кадры совпали бит-в-бит", lists.len());
    assert_no_validation_errors(&context);
}

// ---------------------------------------------------------------------------------------------------------------
// Настоящие сцены продукта: 8 стёкол «типичный кадр камеры» и панель на пол-экрана, 1080 × 2412.
// ---------------------------------------------------------------------------------------------------------------

fn product_screen_backdrop(context: &GpuContext) -> Backdrops {
    Backdrops::new(context, BACKDROP_EXTENT)
}

fn viewfinder_rect(rect: LogicalRect) -> PxRect {
    PxRect::new(
        PxPoint::new(rect.x as f32, rect.y as f32),
        PxSize::new(rect.width as f32, rect.height as f32),
    )
}

fn assert_product_scene_identical(context: &GpuContext, label: &str, list: &DrawList, format: ColorTargetFormat) {
    let backdrops = product_screen_backdrop(context);
    let mut duel = Duel::new(context, (1080, 2412), format);
    let (product, reference) = duel.render(list, backdrops.live(viewfinder_rect(VIEWFINDER)));
    assert!(painted_pixels(&reference) > 50_000, "{label}: сцена пуста");
    assert_bit_identical(label, 1080, &product, &reference);
}

#[test]
fn product_scenes_are_bit_identical_to_frozen_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    assert_product_scene_identical(
        &context,
        "типичный кадр камеры (8 стёкол), fp16",
        &typical_draw_list(),
        ColorTargetFormat::Rgba16Float,
    );
    assert_product_scene_identical(
        &context,
        "типичный кадр камеры (8 стёкол), 10 бит (цель телефона)",
        &typical_draw_list(),
        ColorTargetFormat::A2B10G10R10Unorm,
    );
    assert_product_scene_identical(
        &context,
        "панель на пол-экрана, fp16",
        &panel_draw_list(),
        ColorTargetFormat::Rgba16Float,
    );
    assert_product_scene_identical(
        &context,
        "панель на пол-экрана, 10 бит",
        &panel_draw_list(),
        ColorTargetFormat::A2B10G10R10Unorm,
    );
    assert_no_validation_errors(&context);
}
