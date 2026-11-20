//! Тесты миграции экрана на элементы `ui-kit` (решение 84): порядок слоёв шторки, плавное нажатие кнопки,
//! смена пределов EV без пересоздания рамки фокуса, иконка Level, цвет выбранной подписи и риски шкалы.

use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::exposure::ExposureRangeFact;
use crate::state::{PanelState, ShutterVisibility};
use crate::test_support::live_facts;
use crate::tools::ToolId;

fn near(actual: f32, expected: f32, tolerance: f32) -> bool {
    (actual - expected).abs() <= tolerance
}

/// Индекс первой группы стекла шторки меню (член — скруглённый прямоугольник).
fn drawer_glass_index(list: &DrawList) -> Option<usize> {
    list.items.iter().position(|item| {
        matches!(item, DrawItem::Glass(group)
            if group.members.iter().any(|m| matches!(m.geometry, ShapeGeometry::RoundedRect { .. })))
    })
}

/// Индекс стеклянного члена-круга диаметром около `diameter_px` с центром около (`x_px`, `y_px`).
fn circle_glass_index(list: &DrawList, x_px: f32, y_px: f32, diameter_px: f32) -> Option<usize> {
    list.items.iter().position(|item| {
        matches!(item, DrawItem::Glass(group)
            if group.members.iter().any(|m| matches!(m.geometry, ShapeGeometry::Circle { diameter }
                if near(diameter, diameter_px, 12.0)
                    && near(m.transform.tx, x_px, 12.0)
                    && near(m.transform.ty, y_px, 12.0))))
    })
}

/// Индекс фигуры-круга (белый диск затвора) с центром около (`x_px`, `y_px`).
fn circle_shape_index(list: &DrawList, x_px: f32, y_px: f32) -> Option<usize> {
    list.items.iter().position(|item| {
        matches!(item, DrawItem::Shape(shape)
            if matches!(shape.geometry, ShapeGeometry::Circle { .. })
                && near(shape.transform.tx, x_px, 12.0)
                && near(shape.transform.ty, y_px, 12.0))
    })
}

#[test]
fn dissolving_shutter_is_drawn_entirely_under_the_drawer_glass() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    // Кадры растворения: затвор уже не нажимается, но ещё виден (не дошёл до порога невидимости).
    rig.run(48);
    assert_eq!(
        rig.screen.state().shutter_visibility(),
        ShutterVisibility::HiddenUnderDrawer
    );
    let list = rig.last.clone();
    let drawer = drawer_glass_index(&list).expect("стекло шторки в списке");
    // Затвор по центру экрана: x = 540 px, y ≈ 2034 px, кольцо Ø≈197 px.
    let ring = circle_glass_index(&list, 540.0, 2034.0, 197.0).expect("кольцо затвора ещё рисуется");
    let disc = circle_shape_index(&list, 540.0, 2034.0).expect("белый диск затвора ещё рисуется");
    assert!(
        ring < drawer,
        "кольцо затвора ({ring}) должно лежать под стеклом шторки ({drawer})"
    );
    assert!(
        disc < drawer,
        "диск затвора ({disc}) должен лежать под стеклом шторки ({drawer})"
    );
    // Содержимое кнопки меню (источник морфа) тоже под шторкой, а содержимое пунктов — над её стеклом.
    let menu_icon = list
        .items
        .iter()
        .position(|item| matches!(item, DrawItem::Icon(icon) if icon.icon == IconId::ControlsMenu));
    if let Some(menu_icon) = menu_icon {
        assert!(menu_icon < drawer, "значок кнопки меню под стеклом шторки");
    }
    // Когда шторка раскрылась, содержимое её пунктов лежит над стеклом.
    rig.run(900);
    let list = rig.last.clone();
    let drawer = drawer_glass_index(&list).expect("стекло шторки в списке");
    let flash_item = list
        .items
        .iter()
        .position(|item| matches!(item, DrawItem::Icon(icon) if icon.icon == IconId::Timer))
        .expect("пункт Timer рисуется в шторке");
    assert!(flash_item > drawer, "содержимое шторки лежит поверх её стекла");
}

/// Степень нажатия стеклянного круга с центром около (`x_px`, `y_px`).
fn press_amount_of_circle(list: &DrawList, x_px: f32, y_px: f32) -> f32 {
    list.items
        .iter()
        .find_map(|item| match item {
            DrawItem::Glass(group) => group.members.iter().find_map(|m| match m.geometry {
                ShapeGeometry::Circle { .. }
                    if near(m.transform.tx, x_px, 12.0) && near(m.transform.ty, y_px, 12.0) =>
                {
                    Some(m.press.amount)
                }
                _ => None,
            }),
            _ => None,
        })
        .expect("стеклянный круг кнопки в списке")
}

#[test]
fn plain_button_press_grows_and_releases_smoothly_instead_of_jumping() {
    let mut rig = Rig::new();
    let thumbnail = rig.center_with(|g| g.thumbnail());
    let (x_px, y_px) = (
        (thumbnail.x * f64::from(PX_PER_PT)) as f32,
        (thumbnail.y * f64::from(PX_PER_PT)) as f32,
    );
    assert!(near(press_amount_of_circle(&rig.last, x_px, y_px), 0.0, 1e-6));

    // Палец лёг на миниатюру и держится: степень нажатия растёт по кадрам.
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, thumbnail, start);
    let mut held = Vec::new();
    for _ in 0..30 {
        rig.now_ms += FRAME_MS;
        rig.touch(TouchPhase::Moved, thumbnail, rig.now_ms);
        rig.frame();
        held.push(press_amount_of_circle(&rig.last, x_px, y_px));
    }
    assert!(
        held.windows(2).all(|pair| pair[1] + 1e-4 >= pair[0]),
        "нажатие не убывает при удержании: {held:?}"
    );
    assert!(
        held.iter().any(|amount| *amount > 0.05 && *amount < 0.95),
        "есть промежуточные значения (не скачок 0 → 1): {held:?}"
    );
    assert!(
        held.last().copied().unwrap_or(0.0) > 0.9,
        "через ~0,5 с кнопка почти нажата: {held:?}"
    );

    // Отпустили вне кнопки (без срабатывания): степень плавно падает к нулю.
    let far = Point::new(thumbnail.x, thumbnail.y - 150.0);
    rig.touch(TouchPhase::Moved, far, rig.now_ms + 1);
    rig.touch(TouchPhase::Ended, far, rig.now_ms + 2);
    let mut released = Vec::new();
    for _ in 0..8 {
        rig.now_ms += FRAME_MS;
        rig.frame();
        released.push(press_amount_of_circle(&rig.last, x_px, y_px));
    }
    assert!(
        released.iter().any(|amount| *amount > 0.05 && *amount < 0.95),
        "отпускание тоже плавное: {released:?}"
    );
    rig.run(600);
    assert!(
        near(press_amount_of_circle(&rig.last, x_px, y_px), 0.0, 0.02),
        "кнопка вернулась в покой"
    );
}

#[test]
fn camera_ev_range_change_keeps_the_focus_frame_in_place_and_clamps_the_sun() {
    let mut rig = Rig::new();
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(600);
    // Поднимаем EV на две трети: «солнце» стоит выше середины.
    rig.act(ScreenAction::ExposureRestored {
        bias: camera_contract::control::ExposureBias::from_ev(2.0 / 3.0).expect("EV"),
    });
    rig.run(600);
    assert_eq!(rig.screen.state().exposure().steps(), 2);
    assert!((rig.screen.sun_value_ev() - 2.0 / 3.0).abs() < 0.01);
    let before = rig.last.clone();
    let focus_before = rig.screen.state().focus().indicator();
    assert_eq!(
        text_count_of(&before, "0.7", TextRoleKind::StripValue),
        1,
        "число EV в полосе (два шага по 1/3)"
    );
    assert_eq!(sun_discs(&before).len(), 1, "«солнце» у рамки");

    // Камера сообщила узкий диапазон: максимум 0.3 EV, то есть ни одного шага вверх.
    let mut facts = live_facts();
    facts.exposure_range = ExposureRangeFact::Supported {
        min_ev: -0.3,
        max_ev: 0.3,
    };
    rig.screen.handle_action(ScreenAction::CameraFactsChanged(facts));
    // Один кадр сразу после смены: пересоздание рамки сбросило бы появление (масштаб/прозрачность).
    rig.now_ms += FRAME_MS;
    let first_after = rig.frame().clone();
    let frame_items = |list: &DrawList| {
        list.items
            .iter()
            .filter_map(|item| match item {
                DrawItem::Shape(shape) => Some((shape.transform.tx, shape.transform.ty, shape.opacity.get())),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let before_frame_items = frame_items(&before);
    let after_frame_items = frame_items(&first_after);
    // Рамка (фигуры вокруг центра) осталась на месте и не потускнела.
    let frame_center_px = (
        (center.x * f64::from(PX_PER_PT)) as f32,
        (center.y * f64::from(PX_PER_PT)) as f32,
    );
    let frame_shapes_near = |items: &[(f32, f32, f32)]| {
        items
            .iter()
            .filter(|(x, y, _)| near(*x, frame_center_px.0, 60.0) && near(*y, frame_center_px.1, 60.0))
            .count()
    };
    assert!(
        frame_shapes_near(&before_frame_items) > 0,
        "рамка фокуса нарисована до смены"
    );
    assert_eq!(
        frame_shapes_near(&before_frame_items),
        frame_shapes_near(&after_frame_items),
        "рамка не пересоздавалась"
    );
    for (x, y, opacity) in after_frame_items
        .iter()
        .filter(|(x, y, _)| near(*x, frame_center_px.0, 60.0) && near(*y, frame_center_px.1, 60.0))
    {
        assert!(
            *opacity > 0.9,
            "рамка на месте полностью видна: ({x}, {y}) прозрачность {opacity}"
        );
    }

    rig.run(600);
    assert_eq!(
        rig.screen.state().exposure().steps(),
        0,
        "состояние сжато до нового максимума"
    );
    assert!(
        rig.screen.sun_value_ev().abs() < 0.01,
        "«солнце» сжато вместе с состоянием, а не осталось на {}",
        rig.screen.sun_value_ev()
    );
    assert_eq!(
        rig.screen.state().focus().indicator(),
        focus_before,
        "точка фокуса не менялась"
    );
}

#[test]
fn level_menu_item_is_drawn_with_its_icon() {
    let mut rig = Rig::new();
    assert!(
        rig.screen.state().menu_tools().contains(&ToolId::Level),
        "Level есть в меню v1"
    );
    assert!(
        icon_transforms(&rig.last, IconId::Level).is_empty(),
        "до открытия меню иконки нет"
    );
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    assert_eq!(
        icon_transforms(&rig.last, IconId::Level).len(),
        1,
        "пункт Level рисуется иконкой, а не подписью «LVL»"
    );
}

#[test]
fn selected_mode_label_uses_the_selected_label_color() {
    let mut rig = Rig::new();
    let colors = UiKitTokens::default().colors;
    let capsule_y_px = 2269.0;
    let label = rig
        .frame()
        .items
        .iter()
        .find_map(|item| match item {
            DrawItem::Text(text) if near(text.transform.ty, capsule_y_px, 20.0) => Some(text.clone()),
            _ => None,
        })
        .expect("подпись PHOTO в капсуле");
    assert_eq!(label.color, colors.selected_label, "выбранная подпись карусели жёлтая");
}
