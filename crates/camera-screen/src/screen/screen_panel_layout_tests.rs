//! Панель Exposure и нижняя зона: рамки не пересекаются (затвор остаётся видимым и нажимаемым), а порядок
//! рисования не пускает затвор, кнопку меню и миниатюру поверх стекла панели ни в покое, ни при анимации.

use ui_core::geometry::Rect;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::shape::ShapeGeometry;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::state::PanelState;
use crate::tools::ToolId;

/// Зазор между рамками, pt, ниже которого панель считается «прилипшей» к кольцу затвора.
const MIN_GAP_PT: f64 = 4.0;

fn open_exposure_panel() -> Rig {
    let mut rig = Rig::new();
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig
}

#[test]
fn open_exposure_panel_frame_does_not_overlap_shutter_or_menu_button() {
    let mut rig = open_exposure_panel();
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    let panel = rig.rect_with(|geometry| geometry.exposure_panel());
    let shutter = rig.rect_with(|geometry| geometry.shutter());
    let menu_button = rig.rect_with(|geometry| geometry.menu_button());
    let screen = rig.rect_with(|geometry| geometry.screen());

    assert!(
        !overlaps(&panel, &shutter),
        "панель {panel:?} пересекает затвор {shutter:?}"
    );
    assert!(
        panel.min_y() - shutter.max_y() >= MIN_GAP_PT,
        "зазор между затвором и панелью меньше {MIN_GAP_PT} pt: {panel:?} / {shutter:?}"
    );
    assert!(!overlaps(&panel, &menu_button), "панель пересекает кнопку меню");
    assert!(
        panel.min_x() >= 0.0 && panel.max_x() <= screen.max_x() && panel.max_y() <= screen.max_y(),
        "панель выходит за экран: {panel:?}"
    );
}

/// Пересекаются ли рамки (касание краёв — не пересечение).
fn overlaps(first: &Rect, second: &Rect) -> bool {
    first.min_x() < second.max_x()
        && second.min_x() < first.max_x()
        && first.min_y() < second.max_y()
        && second.min_y() < first.max_y()
}

/// Индексы элементов нижней зоны (кольцо затвора, диск затвора, круглые кнопки меню и миниатюры) в списке.
fn bottom_zone_indices(list: &DrawList) -> Vec<usize> {
    list.items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item {
            DrawItem::Glass(group) => group
                .members
                .iter()
                .any(|member| matches!(member.geometry, ShapeGeometry::Circle { .. }))
                .then_some(index),
            DrawItem::Shape(shape) if matches!(shape.geometry, ShapeGeometry::Circle { diameter } if diameter > 150.0) => {
                Some(index)
            }
            _ => None,
        })
        .collect()
}

/// Индекс стекла панели: первой группы с членом-скруглённым прямоугольником.
fn panel_glass_index(list: &DrawList) -> Option<usize> {
    list.items.iter().position(|item| match item {
        DrawItem::Glass(group) => group
            .members
            .iter()
            .any(|member| matches!(member.geometry, ShapeGeometry::RoundedRect { .. })),
        _ => false,
    })
}

#[test]
fn bottom_zone_items_are_drawn_before_the_exposure_panel_glass_at_every_stage() {
    let mut rig = open_exposure_panel();
    // Открытие (середина анимации), покой и закрытие: порядок не должен зависеть от фазы панели.
    let mut checked_stages = 0;
    let mut check = |rig: &mut Rig, stage: &str| {
        let list = rig.last.clone();
        let panel = panel_glass_index(&list).unwrap_or_else(|| panic!("{stage}: стекла панели нет в списке"));
        let bottom_zone = bottom_zone_indices(&list);
        assert!(
            bottom_zone.len() >= 3,
            "{stage}: элементы нижней зоны не найдены: {bottom_zone:?}"
        );
        assert!(
            bottom_zone.iter().all(|index| *index < panel),
            "{stage}: элемент нижней зоны нарисован поверх стекла панели: {bottom_zone:?}, панель {panel}"
        );
        checked_stages += 1;
    };
    rig.run(64);
    check(&mut rig, "открытие");
    rig.run(900);
    check(&mut rig, "покой");
    rig.act(ScreenAction::ExposurePanelCloseRequested);
    rig.run(64);
    check(&mut rig, "закрытие");
    assert_eq!(checked_stages, 3);
}

#[test]
fn exposure_panel_content_is_drawn_after_its_glass() {
    let mut rig = open_exposure_panel();
    let list = rig.run(900).clone();
    let panel = panel_glass_index(&list).expect("стекло панели");
    let close_icon = list
        .items
        .iter()
        .position(|item| matches!(item, DrawItem::Icon(icon) if icon.icon == ui_draw::icons::IconId::Close))
        .expect("кнопка «×»");
    assert!(close_icon > panel, "содержимое панели стоит позже её стекла");
}

#[test]
fn exposure_panel_vertical_title_fits_inside_the_panel_height() {
    let mut rig = open_exposure_panel();
    let list = rig.run(900).clone();
    let panel: Rect = rig.rect_with(|geometry| geometry.exposure_panel());
    let panel_height_px = (panel.size.height * f64::from(PX_PER_PT)) as f32;
    // Вертикальная подпись EXPOSURE — текст, повёрнутый на 90°: его длина идёт вдоль высоты панели.
    let title_length_px = list
        .items
        .iter()
        .filter_map(|item| match item {
            DrawItem::Text(text) if text.transform.a.abs() < 0.1 && text.transform.b.abs() > 0.9 => {
                Some(text.run.advance_px)
            }
            _ => None,
        })
        .next()
        .expect("повёрнутая подпись EXPOSURE в списке");
    assert!(
        title_length_px < panel_height_px,
        "подпись {title_length_px} px не влезает в панель {panel_height_px} px"
    );
}
