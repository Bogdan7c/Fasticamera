//! Нижняя карточка и нижняя зона: рамки не пересекаются (поднятый затвор над карточкой), порядок рисования не
//! пускает затвор и остатки ряда поверх стекла карточки ни в покое, ни при анимации; содержимое — после стекла.

use ui_core::geometry::Rect;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::state::PanelState;
use crate::tools::ToolId;

/// Зазор между рамками, pt, ниже которого карточка считается «прилипшей» к кольцу затвора.
const MIN_GAP_PT: f64 = 4.0;

fn open_exposure_card() -> Rig {
    let mut rig = Rig::new();
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig
}

#[test]
fn open_card_clears_the_raised_shutter_and_stays_on_screen() {
    let mut rig = open_exposure_card();
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    let card = rig.rect_with(|g| g.card());
    let raised = rig.rect_with(raised_shutter);
    let screen = rig.rect_with(|g| g.screen());
    assert!(
        card.min_y() - raised.max_y() >= MIN_GAP_PT,
        "зазор между поднятым затвором и карточкой меньше {MIN_GAP_PT} pt: {card:?} / {raised:?}"
    );
    assert!(card.min_x() >= 0.0 && card.max_x() <= screen.max_x() && card.max_y() <= screen.max_y());
}

/// Индекс стекла карточки: первой группы с широким скруглённым прямоугольником.
fn card_glass_index(list: &DrawList) -> Option<usize> {
    list.items.iter().position(|item| match item {
        DrawItem::Glass(group) => group
            .members
            .iter()
            .any(|member| matches!(member.geometry, ShapeGeometry::RoundedRect { size, .. } if size.width > 900.0)),
        _ => false,
    })
}

#[test]
fn the_shutter_is_drawn_before_the_card_glass_at_every_stage() {
    let mut rig = open_exposure_card();
    let mut checked = 0;
    let mut check = |rig: &mut Rig, stage: &str| {
        let list = rig.last.clone();
        let card = card_glass_index(&list).unwrap_or_else(|| panic!("{stage}: стекла карточки нет в списке"));
        // Затвор едет (поднимается/опускается), поэтому ищем его кольцо и диск по всему диапазону движения.
        let mut shutter = Vec::new();
        for item_index in 0..list.items.len() {
            shutter.extend(shutter_indices_any_height(&list, item_index));
        }
        assert!(!shutter.is_empty(), "{stage}: затвор не найден");
        assert!(
            shutter.iter().all(|index| *index < card),
            "{stage}: затвор поверх стекла карточки"
        );
        checked += 1;
    };
    rig.run(64);
    check(&mut rig, "открытие");
    rig.run(900);
    check(&mut rig, "покой");
    rig.act(ScreenAction::ExposurePanelCloseRequested);
    rig.run(64);
    check(&mut rig, "закрытие");
    assert_eq!(checked, 3);
}

/// Элемент `index`, если он часть затвора (круг диаметром больше 150 px — кольцо или диск), на любой высоте.
fn shutter_indices_any_height(list: &DrawList, index: usize) -> Option<usize> {
    let is_shutter_circle =
        |geometry: &ShapeGeometry| matches!(geometry, ShapeGeometry::Circle { diameter } if *diameter > 150.0);
    match &list.items[index] {
        DrawItem::Glass(group) if group.members.iter().any(|member| is_shutter_circle(&member.geometry)) => Some(index),
        DrawItem::Shape(shape) if is_shutter_circle(&shape.geometry) => Some(index),
        _ => None,
    }
}

#[test]
fn card_content_is_drawn_after_its_glass_and_the_card_after_the_sheet() {
    let mut rig = Rig::new();
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
    rig.screen
        .handle_action(ScreenAction::ControlsItemSelected(ToolId::Timer));
    // Посередине морфа шторка → карточка и в покое: порядок один и тот же.
    for stage_ms in [120, 900] {
        let list = rig.run(stage_ms).clone();
        let card = card_glass_index(&list).expect("стекло карточки");
        let close_or_chevron = list
            .items
            .iter()
            .position(|item| matches!(item, DrawItem::Icon(icon) if icon.icon == IconId::MenuChevron))
            .expect("«‹» карточки");
        assert!(
            close_or_chevron > card,
            "содержимое карточки после её стекла (+{stage_ms} мс)"
        );
        let sheet_glass = list.items.iter().position(|item| {
            matches!(item, DrawItem::Glass(group)
                if group.members.iter().any(|m| matches!(m.geometry, ShapeGeometry::RoundedRect { size, .. } if size.width > 900.0)))
        });
        assert_eq!(
            sheet_glass,
            Some(card),
            "стекло шторки и карточки — первое широкое стекло, шторка не выше"
        );
    }
}

#[test]
fn card_title_fits_inside_the_card_width() {
    let mut rig = open_exposure_card();
    let list = rig.run(900).clone();
    let card: Rect = rig.rect_with(|g| g.card());
    let width_px = (card.size.width * f64::from(PX_PER_PT)) as f32;
    let widest_text = list
        .items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Text(text) => Some(text.run.advance_px),
            _ => None,
        })
        .fold(0.0_f32, f32::max);
    assert!(
        widest_text > 0.0 && widest_text < width_px / 2.0,
        "текст {widest_text} px в карточке {width_px} px"
    );
}
