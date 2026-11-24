//! Прямоугольная обрезка (`DrawItem::Clipped`): фигура, текст, иконка, картинка и стекло, пересекающие край
//! обрезки, режутся ровно по правилу центра пикселя (`ui_draw::draw_list::clip_keeps_pixel`).
//!
//! Метод проверки (по трём кадрам одной сцены, побитно):
//! * «пусто» — сцена без проверяемого содержимого;
//! * «целиком» — то же содержимое без обрезки;
//! * «обрезано» — содержимое внутри группы обрезки.
//!
//! Каждый пиксель вне обрезки в кадре «обрезано» обязан совпасть с «пусто», внутри — с «целиком». После группы
//! рисуется необрезанная фигура: если бы scissor «протёк» на следующие вызовы, она оказалась бы обрезанной.
//! Принадлежность пикселя обрезке тест считает сам по правилу `clip_keeps_pixel` и независимой раскладке
//! поворота цели, а не берёт у продукта пиксельный прямоугольник.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::pictures::{gradient_picture, image_item};
use support::{Rig, assert_no_validation_errors, empty_list, glass_group, gpu_context_or_skip, shape, shape_item};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{ClipGroup, DrawItem, DrawList, IconItem, TextItem, clip_keeps_pixel};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::icons::IconId;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};
use ui_draw::transform::Affine2;
use ui_render_vulkan::BackdropInput;

/// Логический размер экрана (ширина, высота).
const LOGICAL: (u32, u32) = (120, 160);
const BACKDROP: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor {
        r: 0.25,
        g: 0.4,
        b: 0.55,
        a: 1.0,
    },
};

/// Все повороты цели: (преобразование поверхности, число четвертей).
const ROTATIONS: [(SurfaceTransform, u32); 4] = [
    (SurfaceTransform::Identity, 0),
    (SurfaceTransform::Rotate90, 1),
    (SurfaceTransform::Rotate180, 2),
    (SurfaceTransform::Rotate270, 3),
];

fn rect(x: f32, y: f32, width: f32, height: f32) -> PxRect {
    PxRect::new(PxPoint::new(x, y), PxSize::new(width, height))
}

/// Прямоугольники обрезки: целые границы, дробные и ровно на половине пикселя (ничья правила).
fn clip_variants() -> Vec<(&'static str, PxRect)> {
    vec![
        ("целые границы", rect(30.0, 40.0, 60.0, 80.0)),
        ("дробные границы", rect(30.3, 40.6, 59.9, 79.6)),
        ("границы на .5", rect(30.5, 40.5, 59.0, 80.0)),
    ]
}

/// Что именно рисуется сквозь обрезку.
#[derive(Clone, Copy, Debug)]
enum Content {
    Shape,
    Text,
    Icon,
    Image,
    Glass,
}

/// Элементы содержимого: каждый заведомо пересекает все четыре края самой широкой обрезки или хотя бы
/// левый и правый (текст, стекло).
fn content_items(content: Content) -> Vec<DrawItem> {
    match content {
        Content::Shape => vec![shape(shape_item(
            ShapeGeometry::RoundedRect {
                size: PxSize::new(100.0, 110.0),
                corner: CornerSpec::continuous(18.0),
            },
            Affine2::translate(60.0, 80.0),
            DisplayColor::rgb(0.9, 0.3, 0.2),
        ))],
        Content::Text => {
            let style = TextStyle {
                face: FontFace::Semibold,
                size_px: 56.0,
                tracking_em: 0.0,
                figures: Figures::Proportional,
            };
            vec![DrawItem::Text(TextItem {
                run: layout_line("Clip", &style).unwrap(),
                transform: Affine2::translate(4.0, 95.0),
                color: DisplayColor::WHITE,
                opacity: Opacity::OPAQUE,
            })]
        }
        Content::Icon => vec![DrawItem::Icon(IconItem {
            icon: IconId::Grid,
            transform: Affine2::translate(60.0, 80.0),
            size_px: PxSize::new(110.0, 110.0),
            color: DisplayColor::WHITE,
            opacity: Opacity::OPAQUE,
        })],
        Content::Image => {
            let picture = gradient_picture(24, 24);
            vec![DrawItem::Image(image_item(
                &picture,
                (110.0, 110.0),
                Affine2::translate(60.0, 80.0),
                1.0,
            ))]
        }
        Content::Glass => {
            let geometry = ShapeGeometry::RoundedRect {
                size: PxSize::new(100.0, 70.0),
                corner: CornerSpec::continuous(24.0),
            };
            vec![DrawItem::Glass(glass_group(
                &GlassStyle::default(),
                &[(geometry, PxPoint::new(60.0, 80.0))],
                0.0,
            ))]
        }
    }
}

/// Фон сцены: две фигуры, видимые под содержимым.
fn backdrop_shapes() -> Vec<DrawItem> {
    vec![
        shape(shape_item(
            ShapeGeometry::Circle { diameter: 90.0 },
            Affine2::translate(40.0, 50.0),
            DisplayColor::rgb(0.2, 0.7, 0.4),
        )),
        shape(shape_item(
            ShapeGeometry::Capsule {
                size: PxSize::new(110.0, 30.0),
            },
            Affine2::translate(60.0, 120.0),
            DisplayColor::rgb(0.8, 0.8, 0.2),
        )),
    ]
}

/// Необрезанная фигура ПОСЛЕ группы: перекрывает область обрезки и выходит за неё. Если обрезка протекла на
/// следующий вызов рисования, эта фигура окажется обрезанной и кадр разойдётся с эталонами.
fn trailing_shape() -> DrawItem {
    shape(shape_item(
        ShapeGeometry::Circle { diameter: 70.0 },
        Affine2::translate(95.0, 125.0),
        DisplayColor::rgb(0.6, 0.3, 0.9),
    ))
}

fn list_of(items: Vec<DrawItem>) -> DrawList {
    let mut list = empty_list();
    list.items = items;
    list
}

/// Три кадра сцены: без содержимого, с содержимым без обрезки и с содержимым в обрезке `wrap(content)`.
struct Frames {
    empty: Vec<[f32; 4]>,
    unclipped: Vec<[f32; 4]>,
    clipped: Vec<[f32; 4]>,
}

fn render_frames(rig: &mut Rig, content: &[DrawItem], wrap: impl FnOnce(Vec<DrawItem>) -> DrawItem) -> Frames {
    let scene = |middle: Vec<DrawItem>| {
        let mut items = backdrop_shapes();
        items.extend(middle);
        items.push(trailing_shape());
        list_of(items)
    };
    let empty = rig.render(&scene(Vec::new()), BACKDROP).1;
    let unclipped = rig.render(&scene(content.to_vec()), BACKDROP).1;
    let clipped = rig.render(&scene(vec![wrap(content.to_vec())]), BACKDROP).1;
    Frames {
        empty,
        unclipped,
        clipped,
    }
}

/// Индекс физического пикселя, в который попадает логический пиксель `(x, y)` при повороте цели на
/// `quarter_turns` (то же соответствие, что у `logical_to_physical` в `ui_frame.glsl`, но для целых индексов).
fn physical_index(quarter_turns: u32, x: u32, y: u32) -> usize {
    let (width, height) = LOGICAL;
    // Физическая ширина цели: при повороте на 90°/270° это логическая высота.
    let (physical_x, physical_y, physical_width) = match quarter_turns {
        1 => (y, width - 1 - x, height),
        2 => (width - 1 - x, height - 1 - y, width),
        3 => (height - 1 - y, x, height),
        _ => (x, y, width),
    };
    (physical_y * physical_width + physical_x) as usize
}

fn bits(pixel: &[f32; 4]) -> [u32; 4] {
    pixel.map(f32::to_bits)
}

/// Сверка трёх кадров по правилу центра пикселя для набора вложенных прямоугольников (пиксель виден, если его
/// центр лежит во ВСЕХ). Возвращает (сколько пикселей содержимого видно внутри, сколько попало бы наружу):
/// оба числа обязаны быть ненулевыми, иначе тест ничего не доказывает.
fn assert_cut_by_pixel_rule(label: &str, quarter_turns: u32, clips: &[PxRect], frames: &Frames) -> (usize, usize) {
    let (mut visible_inside, mut would_leak_outside) = (0_usize, 0_usize);
    for y in 0..LOGICAL.1 {
        for x in 0..LOGICAL.0 {
            let index = physical_index(quarter_turns, x, y);
            let center = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
            let inside = clips.iter().all(|clip| clip_keeps_pixel(clip, center));
            let content_changes_pixel = bits(&frames.unclipped[index]) != bits(&frames.empty[index]);
            let (expected, source) = if inside {
                (&frames.unclipped[index], "целиком")
            } else {
                (&frames.empty[index], "пусто")
            };
            assert_eq!(
                bits(&frames.clipped[index]),
                bits(expected),
                "{label}, поворот {quarter_turns}: пиксель ({x}, {y}) {} обрезки должен совпасть с кадром «{source}»",
                if inside { "внутри" } else { "вне" }
            );
            if content_changes_pixel {
                if inside {
                    visible_inside += 1;
                } else {
                    would_leak_outside += 1;
                }
            }
        }
    }
    (visible_inside, would_leak_outside)
}

/// Прогон содержимого `content` через все повороты и все варианты прямоугольника обрезки.
fn check_content(content: Content) {
    let Some(context) = gpu_context_or_skip() else { return };
    let items = content_items(content);
    for (transform, quarter_turns) in ROTATIONS {
        let mut rig = Rig::new(&context, LOGICAL, transform, ColorTargetFormat::Rgba8Unorm);
        for (name, clip) in clip_variants() {
            let frames = render_frames(&mut rig, &items, |children| {
                DrawItem::Clipped(ClipGroup {
                    rect: clip,
                    items: children,
                })
            });
            let label = format!("{content:?}, {name}");
            let (inside, outside) = assert_cut_by_pixel_rule(&label, quarter_turns, &[clip], &frames);
            assert!(
                inside > 100,
                "{label}: внутри обрезки видно лишь {inside} пикселей содержимого"
            );
            assert!(
                outside > 100,
                "{label}: без обрезки содержимое залило бы {outside} пикселей вне её — тест слишком слаб"
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn shape_crossing_the_clip_edge_is_cut_by_the_pixel_rule() {
    check_content(Content::Shape);
}

#[test]
fn text_crossing_the_clip_edge_is_cut_by_the_pixel_rule() {
    check_content(Content::Text);
}

#[test]
fn icon_crossing_the_clip_edge_is_cut_by_the_pixel_rule() {
    check_content(Content::Icon);
}

#[test]
fn image_crossing_the_clip_edge_is_cut_by_the_pixel_rule() {
    check_content(Content::Image);
}

#[test]
fn glass_crossing_the_clip_edge_is_cut_by_the_pixel_rule() {
    check_content(Content::Glass);
}

/// Все виды содержимого в одной группе: пакеты разных видов под одной обрезкой.
#[test]
fn mixed_content_in_one_clip_group_is_cut_by_the_pixel_rule() {
    let Some(context) = gpu_context_or_skip() else { return };
    let items: Vec<DrawItem> = [
        Content::Shape,
        Content::Glass,
        Content::Text,
        Content::Icon,
        Content::Image,
    ]
    .into_iter()
    .flat_map(content_items)
    .collect();
    let clip = rect(30.3, 40.6, 59.9, 79.6);
    for (transform, quarter_turns) in ROTATIONS {
        let mut rig = Rig::new(&context, LOGICAL, transform, ColorTargetFormat::Rgba8Unorm);
        let frames = render_frames(&mut rig, &items, |children| {
            DrawItem::Clipped(ClipGroup {
                rect: clip,
                items: children,
            })
        });
        assert_cut_by_pixel_rule("смесь", quarter_turns, &[clip], &frames);
    }
    assert_no_validation_errors(&context);
}

/// Вложенные группы режут по пересечению прямоугольников.
#[test]
fn nested_clip_groups_cut_by_the_intersection_of_their_rects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let outer = rect(20.4, 30.0, 70.0, 100.0);
    let inner = rect(40.0, 50.6, 70.0, 90.0);
    for content in [Content::Shape, Content::Glass] {
        let items = content_items(content);
        for (transform, quarter_turns) in ROTATIONS {
            let mut rig = Rig::new(&context, LOGICAL, transform, ColorTargetFormat::Rgba8Unorm);
            let frames = render_frames(&mut rig, &items, |children| {
                DrawItem::Clipped(ClipGroup {
                    rect: outer,
                    items: vec![DrawItem::Clipped(ClipGroup {
                        rect: inner,
                        items: children,
                    })],
                })
            });
            let label = format!("вложенные, {content:?}");
            let (inside, outside) = assert_cut_by_pixel_rule(&label, quarter_turns, &[outer, inner], &frames);
            assert!(
                inside > 100 && outside > 100,
                "{label}: слабый тест ({inside}, {outside})"
            );
        }
    }
    assert_no_validation_errors(&context);
}

/// Пустая обрезка (нулевой размер или ни один центр пикселя не попал) ничего не рисует.
#[test]
fn empty_clip_draws_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let empty_clips = [
        rect(50.0, 60.0, 0.0, 40.0),    // нулевая ширина
        rect(50.0, 60.0, 40.0, 0.0),    // нулевая высота
        rect(50.6, 60.0, 0.3, 40.0),    // узкая щель без центров пикселей: [50,6; 50,9)
        rect(500.0, 500.0, 20.0, 20.0), // целиком вне цели
    ];
    for content in [Content::Shape, Content::Text, Content::Glass] {
        let items = content_items(content);
        for (transform, _) in ROTATIONS {
            let mut rig = Rig::new(&context, LOGICAL, transform, ColorTargetFormat::Rgba8Unorm);
            for clip in empty_clips {
                let frames = render_frames(&mut rig, &items, |children| {
                    DrawItem::Clipped(ClipGroup {
                        rect: clip,
                        items: children,
                    })
                });
                assert!(
                    frames
                        .clipped
                        .iter()
                        .zip(&frames.empty)
                        .all(|(a, b)| bits(a) == bits(b)),
                    "{content:?}, обрезка {clip:?}: пустая обрезка изменила кадр"
                );
                assert!(
                    frames
                        .unclipped
                        .iter()
                        .zip(&frames.empty)
                        .any(|(a, b)| bits(a) != bits(b)),
                    "страховка: содержимое без обрезки должно быть видно"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

/// Обрезанные и необрезанные элементы вперемешку: scissor выставляется на каждый вызов и не остаётся от
/// прошлого. Порядок рисования сохраняется (обрезанная группа лежит между необрезанными).
#[test]
fn clip_does_not_leak_into_neighbouring_items_and_keeps_paint_order() {
    let Some(context) = gpu_context_or_skip() else { return };
    let clip_a = rect(10.0, 10.0, 40.0, 50.0);
    let clip_b = rect(70.0, 90.0, 40.0, 60.0);
    let big = |color: DisplayColor, x: f32, y: f32| {
        shape(shape_item(
            ShapeGeometry::Circle { diameter: 90.0 },
            Affine2::translate(x, y),
            color,
        ))
    };
    let red = DisplayColor::rgb(0.9, 0.2, 0.2);
    let blue = DisplayColor::rgb(0.2, 0.3, 0.9);
    let green = DisplayColor::rgb(0.2, 0.8, 0.3);
    // красный (без обрезки) — синий в обрезке A — зелёный в обрезке B — красный ещё раз без обрезки.
    let mixed = |wrap: bool| {
        let group = |rect: PxRect, item: DrawItem| {
            if wrap {
                DrawItem::Clipped(ClipGroup {
                    rect,
                    items: vec![item],
                })
            } else {
                item
            }
        };
        list_of(vec![
            big(red, 30.0, 30.0),
            group(clip_a, big(blue, 40.0, 50.0)),
            group(clip_b, big(green, 80.0, 110.0)),
            big(red, 100.0, 20.0),
        ])
    };
    for (transform, quarter_turns) in ROTATIONS {
        let mut rig = Rig::new(&context, LOGICAL, transform, ColorTargetFormat::Rgba8Unorm);
        let unclipped = rig.render(&mixed(false), BACKDROP).1;
        let clipped = rig.render(&mixed(true), BACKDROP).1;
        // Без группы B и A вовсе: те же четыре круга, но синий и зелёный отсутствуют.
        let without = rig
            .render(&list_of(vec![big(red, 30.0, 30.0), big(red, 100.0, 20.0)]), BACKDROP)
            .1;
        let (mut differs_from_unclipped, mut matches_without_somewhere) = (0, 0);
        for y in 0..LOGICAL.1 {
            for x in 0..LOGICAL.0 {
                let index = physical_index(quarter_turns, x, y);
                let center = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
                let in_a = clip_keeps_pixel(&clip_a, center);
                let in_b = clip_keeps_pixel(&clip_b, center);
                if !in_a && !in_b {
                    // Вне обеих обрезок синий и зелёный не видны: кадр равен «без них».
                    assert_eq!(bits(&clipped[index]), bits(&without[index]), "({x}, {y}) вне обрезок");
                    matches_without_somewhere += 1;
                }
                if bits(&clipped[index]) != bits(&unclipped[index]) {
                    differs_from_unclipped += 1;
                }
            }
        }
        assert!(matches_without_somewhere > 1000);
        assert!(differs_from_unclipped > 100, "обрезка должна менять кадр");
        // Внутри A синий лежит поверх красного, а красный круг после групп (100, 20) — поверх всего.
        let center_red_after = physical_index(quarter_turns, 100, 20);
        assert_eq!(bits(&clipped[center_red_after]), bits(&unclipped[center_red_after]));
    }
    assert_no_validation_errors(&context);
}
