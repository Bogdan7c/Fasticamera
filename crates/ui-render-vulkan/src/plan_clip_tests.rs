//! Тесты плана кадра с группами обрезки: пакеты делятся только по смене обрезки, невидимые листья не
//! упаковываются и не занимают пределы, стекло получает scissor «квад ∩ обрезка», а вершины не меняются.

#![allow(clippy::unwrap_used, clippy::panic, reason = "тесты")]

use super::*;
use crate::pack::SHAPE_INSTANCE_BYTES;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{
    ClipGroup, GlassGroupItem, GlassMember, ImageClip, ImageFit, ImageItem, PressState, ShapeItem,
};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::light::LightDirection;
use ui_draw::picture::{PictureEncoding, PictureSize};
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

fn rect(x: f32, y: f32, width: f32, height: f32) -> PxRect {
    PxRect::new(PxPoint::new(x, y), PxSize::new(width, height))
}

/// Круг диаметром 10 с центром в `(x, y)`.
fn shape_at(x: f32, y: f32) -> DrawItem {
    DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle { diameter: 10.0 },
        transform: Affine2::translate(x, y),
        fill: DisplayColor::WHITE,
        stroke: None,
        shadow: None,
        opacity: Opacity::OPAQUE,
    })
}

fn glass_at(x: f32, y: f32) -> DrawItem {
    let size = PxSize::new(40.0, 20.0);
    DrawItem::Glass(GlassGroupItem {
        params: GlassStyle::default().resolve(size, 2.0),
        members: vec![GlassMember {
            geometry: ShapeGeometry::Capsule { size },
            transform: Affine2::translate(x, y),
            press: PressState::RELEASED,
        }],
        merge_radius_px: 0.0,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    })
}

fn clipped(rect: PxRect, items: Vec<DrawItem>) -> DrawItem {
    DrawItem::Clipped(ClipGroup { rect, items })
}

fn plan_of(items: Vec<DrawItem>, limits: &UiRenderLimits) -> Result<FramePlan, UiRenderError> {
    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    list.items = items;
    build_plan(&list, limits, LiveImages::none())
}

fn plan(items: Vec<DrawItem>) -> FramePlan {
    plan_of(items, &UiRenderLimits::default()).unwrap()
}

fn batch(kind: BatchKind, clip: BatchClip, first_instance: u32, instance_count: u32) -> Batch {
    Batch {
        kind,
        clip,
        first_instance,
        instance_count,
    }
}

#[test]
fn batches_split_only_when_the_clip_changes() {
    // Дробные границы: центры пикселей 10..41 (10,3 -> 10; 40,6 -> 41).
    let first_clip = rect(10.3, 5.0, 30.3, 50.0);
    let second_clip = rect(0.0, 0.0, 100.0, 100.0);
    let plan = plan(vec![
        clipped(first_clip, vec![shape_at(20.0, 20.0), shape_at(30.0, 20.0)]),
        // Тот же вид, другая обрезка: новый пакет.
        clipped(second_clip, vec![shape_at(50.0, 50.0)]),
        // Без обрезки: ещё один пакет, два соседних склеиваются.
        shape_at(60.0, 60.0),
        shape_at(70.0, 60.0),
    ]);
    let first_pixels = BatchClip::PixelRect(rect(10.0, 5.0, 31.0, 50.0));
    let second_pixels = BatchClip::PixelRect(rect(0.0, 0.0, 100.0, 100.0));
    assert_eq!(
        plan.batches,
        vec![
            batch(BatchKind::Shape, first_pixels, 0, 2),
            batch(BatchKind::Shape, second_pixels, 2, 1),
            batch(BatchKind::Shape, BatchClip::WholeTarget, 3, 2),
        ]
    );
    assert_eq!(plan.shape_count, 5);
    assert_eq!(plan.shape_bytes.len(), 5 * SHAPE_INSTANCE_BYTES);
}

#[test]
fn adjacent_groups_with_equal_clip_merge_into_one_call() {
    let clip = rect(0.0, 0.0, 100.0, 100.0);
    let plan = plan(vec![
        clipped(clip, vec![shape_at(20.0, 20.0)]),
        clipped(clip, vec![shape_at(30.0, 20.0)]),
    ]);
    assert_eq!(plan.batches.len(), 1, "две группы с одним прямоугольником — один вызов");
    assert_eq!(plan.batches[0].instance_count, 2);
}

#[test]
fn unclipped_list_plans_exactly_like_the_same_items_inside_a_covering_clip() {
    let items = || vec![shape_at(20.0, 20.0), glass_at(60.0, 40.0), shape_at(30.0, 20.0)];
    let flat = plan(items());
    assert!(flat.batches.iter().all(|batch| batch.clip == BatchClip::WholeTarget));
    let wrapped = plan(vec![clipped(rect(-1000.0, -1000.0, 3000.0, 3000.0), items())]);
    // Упакованные байты и счётчики те же: обрезка меняет только scissor пакетов.
    assert_eq!(flat.shape_bytes, wrapped.shape_bytes);
    assert_eq!(flat.group_bytes, wrapped.group_bytes);
    assert_eq!(flat.member_bytes, wrapped.member_bytes);
    assert_eq!(
        (flat.shape_count, flat.group_count),
        (wrapped.shape_count, wrapped.group_count)
    );
    let shape_of = |plan: &FramePlan| {
        plan.batches
            .iter()
            .map(|batch| (batch.kind, batch.first_instance, batch.instance_count))
            .collect::<Vec<_>>()
    };
    assert_eq!(shape_of(&flat), shape_of(&wrapped));
    // Квад стекла внутри такой обрезки не режется.
    assert_eq!(flat.glass_scissors, wrapped.glass_scissors);
}

#[test]
fn invisible_leaves_are_not_packed_and_do_not_split_neighbouring_batches() {
    let clip = rect(0.0, 0.0, 50.0, 50.0);
    let plan = plan(vec![
        clipped(
            clip,
            vec![
                shape_at(10.0, 10.0),
                shape_at(500.0, 500.0), // целиком вне обрезки
                shape_at(20.0, 10.0),
            ],
        ),
        // Схлопнутая обрезка: всё внутри невидимо.
        clipped(rect(5.0, 5.0, 0.0, 40.0), vec![shape_at(10.0, 10.0)]),
    ]);
    assert_eq!(plan.shape_count, 2);
    assert_eq!(plan.shape_bytes.len(), 2 * SHAPE_INSTANCE_BYTES);
    assert_eq!(
        plan.batches,
        vec![batch(
            BatchKind::Shape,
            BatchClip::PixelRect(rect(0.0, 0.0, 50.0, 50.0)),
            0,
            2
        )]
    );
}

#[test]
fn fully_clipped_out_list_makes_an_empty_plan() {
    let plan = plan(vec![clipped(rect(0.0, 0.0, 10.0, 10.0), vec![shape_at(300.0, 300.0)])]);
    assert!(plan.batches.is_empty());
    assert_eq!((plan.shape_count, plan.shape_bytes.len()), (0, 0));
}

#[test]
fn clipped_out_leaves_do_not_consume_limits() {
    let limits = UiRenderLimits {
        max_shape_instances: 2,
        ..Default::default()
    };
    let clip = rect(0.0, 0.0, 50.0, 50.0);
    // Шесть фигур, видимы две: предел 2 не превышен.
    let mut items = vec![shape_at(10.0, 10.0), shape_at(20.0, 10.0)];
    items.extend((0..4).map(|n| shape_at(200.0 + 20.0 * n as f32, 10.0)));
    let accepted = plan_of(vec![clipped(clip, items)], &limits).unwrap();
    assert_eq!(accepted.shape_count, 2);
    // Три видимых — предел превышен, и число в ошибке то же, что считает план.
    let three_visible = vec![shape_at(10.0, 10.0), shape_at(20.0, 10.0), shape_at(30.0, 10.0)];
    let error = plan_of(vec![clipped(clip, three_visible)], &limits).unwrap_err();
    assert!(
        matches!(
            error,
            UiRenderError::LimitExceeded {
                kind: LimitKind::ShapeInstances,
                requested: 3,
                limit: 2
            }
        ),
        "{error:?}"
    );
}

#[test]
fn clipped_out_picture_takes_neither_a_sampler_nor_a_size_check() {
    let huge = UiPicture::from_rgba8(
        PictureSize::new(2000, 10),
        PictureEncoding::Srgb,
        vec![0; 2000 * 10 * 4],
    )
    .unwrap();
    let image = DrawItem::Image(ImageItem {
        source: huge.into(),
        transform: Affine2::translate(300.0, 300.0),
        size_px: PxSize::new(20.0, 20.0),
        fit: ImageFit::Cover,
        clip: ImageClip::None,
        opacity: Opacity::OPAQUE,
    });
    let plan = plan(vec![clipped(rect(0.0, 0.0, 50.0, 50.0), vec![image])]);
    assert_eq!((plan.image_count, plan.sampler_slots.len()), (0, 0));
}

#[test]
fn glass_scissor_is_draw_rect_intersected_with_clip_while_vertices_stay() {
    let group_item = glass_at(60.0, 40.0);
    let DrawItem::Glass(group) = &group_item else {
        panic!("ожидали стекло");
    };
    let draw_rect = group.draw_rect_px().unwrap();
    let unclipped = plan(vec![group_item.clone()]);
    assert_eq!(unclipped.glass_scissors, vec![draw_rect]);

    // Обрезка режет квад по x (дробная граница 60,3 -> пиксели до 60).
    let clip = rect(0.0, 0.0, 60.3, 200.0);
    let clipped_plan = plan(vec![clipped(clip, vec![group_item])]);
    let pixels = BatchClip::PixelRect(rect(0.0, 0.0, 60.0, 200.0));
    assert_eq!(clipped_plan.batches, vec![batch(BatchKind::Glass, pixels, 0, 1)]);
    let expected = draw_rect.intersection(&rect(0.0, 0.0, 60.0, 200.0)).unwrap();
    assert_eq!(clipped_plan.glass_scissors, vec![expected]);
    assert!(expected.size.width < draw_rect.size.width);
    // Вершины квада и область адаптации яркости — те же байты, что без обрезки.
    assert_eq!(clipped_plan.group_bytes, unclipped.group_bytes);
    assert_eq!(clipped_plan.member_bytes, unclipped.member_bytes);
}

#[test]
fn glass_fully_outside_the_clip_is_skipped_and_numbering_stays_aligned() {
    let plan = plan(vec![
        clipped(rect(0.0, 0.0, 30.0, 30.0), vec![glass_at(300.0, 300.0)]),
        glass_at(60.0, 40.0),
    ]);
    assert_eq!(plan.group_count, 1, "скрытая группа не упакована");
    assert_eq!(plan.glass_scissors.len(), 1, "номер группы = номер записи scissor");
}

#[test]
fn nested_clips_collapse_into_the_intersection_of_pixel_rects() {
    let plan = plan(vec![clipped(
        rect(0.0, 0.0, 50.0, 50.0),
        vec![clipped(rect(20.0, 10.0, 100.0, 100.0), vec![shape_at(30.0, 30.0)])],
    )]);
    assert_eq!(
        plan.batches,
        vec![batch(
            BatchKind::Shape,
            BatchClip::PixelRect(rect(20.0, 10.0, 30.0, 40.0)),
            0,
            1
        )]
    );
}
