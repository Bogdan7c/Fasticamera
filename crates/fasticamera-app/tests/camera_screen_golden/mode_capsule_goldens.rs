//! Капсула режимов (решение 232): подпись соседнего режима режется ровно по стеклу капсулы — и на PHOTO, и в CALIBRATE.

use shell_contract::orientation::DeviceOrientation;
use ui_draw::draw_list::{ClipGroup, DrawItem, DrawList};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};

use crate::calibration_goldens::{calibration, render_calibration_list};
use crate::driver::{HEIGHT_PX, WIDTH_PX};
use crate::render_rig::{RenderRig, ScopeImage, Snapshot};
use crate::{states, with_rig};

/// Тот же список, но группы обрезки раскрыты на весь экран: как рисовалось бы без обрезки.
fn without_clipping(list: &DrawList) -> DrawList {
    let whole_screen = PxRect::new(PxPoint::new(0.0, 0.0), PxSize::new(WIDTH_PX as f32, HEIGHT_PX as f32));
    let items = list
        .items
        .iter()
        .map(|item| match item {
            DrawItem::Clipped(group) => DrawItem::Clipped(ClipGroup {
                rect: whole_screen,
                items: group.items.clone(),
            }),
            other => other.clone(),
        })
        .collect();
    DrawList {
        items,
        light: list.light,
    }
}

/// Прямоугольник обрезки подписей режимов: группа, чей текст стоит в нижнем ряду экрана.
fn capsule_clip(list: &DrawList) -> PxRect {
    let bottom_row_top = HEIGHT_PX as f32 * 0.85;
    list.items
        .iter()
        .rev()
        .find_map(|item| match item {
            DrawItem::Clipped(group) if group.rect.origin.y > bottom_row_top => Some(group.rect),
            _ => None,
        })
        .expect("подписи капсулы лежат в группе обрезки")
}

/// Сколько пикселей нижнего ряда отличаются от `other` вне прямоугольника `clip` (по высоте капсулы ± 40 px).
fn differing_outside(first: &Snapshot, other: &Snapshot, clip: &PxRect) -> usize {
    let (top, bottom) = (clip.origin.y as u32 - 40, (clip.max_y() as u32 + 40).min(HEIGHT_PX));
    let inside = |x: u32, y: u32| {
        (clip.origin.x..clip.max_x()).contains(&(x as f32 + 0.5))
            && (clip.origin.y..clip.max_y()).contains(&(y as f32 + 0.5))
    };
    (top..bottom)
        .flat_map(|y| (0..WIDTH_PX).map(move |x| (x, y)))
        .filter(|(x, y)| !inside(*x, *y) && first.at(*x, *y) != other.at(*x, *y))
        .count()
}

/// Список без подписей капсулы: эталон «вне капсулы ничего не нарисовано».
fn without_labels(list: &DrawList, clip: &PxRect) -> DrawList {
    DrawList {
        items: list
            .items
            .iter()
            .filter(|item| !matches!(item, DrawItem::Clipped(group) if group.rect == *clip))
            .cloned()
            .collect(),
        light: list.light,
    }
}

/// Проверяет один экран: подписи вне капсулы не оставляют ни пикселя, а без обрезки оставляли бы.
fn assert_labels_end_at_the_glass(rig: &mut RenderRig, list: &DrawList, context: &str) {
    let mut draw = |list: &DrawList| render_calibration_list(rig, list, ScopeImage::StandIn);
    let clip = capsule_clip(list);
    let bare = draw(&without_labels(list, &clip));
    let shown = draw(list);
    assert_eq!(
        differing_outside(&shown, &bare, &clip),
        0,
        "{context}: вне капсулы подписи режимов не рисуются"
    );
    let unclipped = draw(&without_clipping(list));
    assert!(
        differing_outside(&unclipped, &bare, &clip) > 100,
        "{context}: контроль — без обрезки сосед вылезал бы за стекло"
    );
}

#[test]
fn the_neighbour_mode_label_is_cut_exactly_at_the_capsule_glass_in_photo_and_calibrate() {
    with_rig(|rig| {
        assert_labels_end_at_the_glass(rig, states::rest().list(), "PHOTO");
        assert_labels_end_at_the_glass(rig, calibration(DeviceOrientation::Portrait).list(), "CALIBRATE");
    });
}
