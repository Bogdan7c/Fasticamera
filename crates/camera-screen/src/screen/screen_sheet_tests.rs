//! Шторка Controls, верхний ряд и сетка в числах iOS 27 (пакет P3b) через настоящие кадры экрана 1080 × 2412 px:
//! рамка, радиус, «ручка», круги и подписи пунктов, текст EV, пилюля режима, линия сетки, морф шторки в карточку.

use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::settings::GridSetting;
use crate::tools::{ToolId, ToolOption};

/// Допуск сравнения координат, px.
const PX_TOLERANCE: f32 = 1.5;
/// Сдвиг эталонных y вниз: высота нашего экрана в pt минус 956.
fn bottom_shift_pt() -> f64 {
    f64::from(HEIGHT_PX) * 440.0 / f64::from(WIDTH_PX) - 956.0
}

fn open_menu(rig: &mut Rig) {
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
}

/// Включает сетку настоящим путём: меню → карточка Grid → вариант «On» → «‹» обратно в меню.
fn enable_grid_and_return_to_menu(rig: &mut Rig) {
    open_menu(rig);
    rig.act(ScreenAction::ControlsItemSelected(ToolId::Grid));
    rig.act(ScreenAction::SubmenuOptionSelected(ToolOption::Grid(GridSetting::On)));
    rig.act(ScreenAction::SubmenuBackRequested);
    rig.run(700);
    assert_eq!(rig.screen.state().settings().grid, GridSetting::On);
}

/// Все скруглённые прямоугольники панелей (силуэты вместе с ободком): (размер px, запрошенный радиус px, центр y px).
fn rounded_panels(list: &DrawList) -> Vec<(ui_draw::geometry::PxSize, f32, f32)> {
    panel_members(list)
        .into_iter()
        .filter_map(|(geometry, _, ty)| match geometry {
            ShapeGeometry::RoundedRect { size, corner } => Some((size, corner.radius_px, ty)),
            _ => None,
        })
        .collect()
}

/// Фигуры-круги Ø `diameter_pt` с заливкой: (центр x px, центр y px, заливка).
fn circles(list: &DrawList, diameter_pt: f64) -> Vec<(f32, f32, DisplayColor)> {
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape) => match shape.geometry {
                ShapeGeometry::Circle { diameter } if near(diameter, px(diameter_pt), 0.6) => {
                    Some((shape.transform.tx, shape.transform.ty, shape.fill))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn open_sheet_sits_at_the_ios27_numbers_with_black_circles_and_a_handle() {
    let mut rig = Rig::new();
    open_menu(&mut rig);
    let sheet = rig.rect_with(|g| g.sheet());
    // [measured: C t=30.0] рамка 430 × 464 pt, радиус 54 pt, низ на 8,6 pt выше низа экрана.
    let screen_height_pt = f64::from(HEIGHT_PX) * 440.0 / f64::from(WIDTH_PX);
    assert!((sheet.max_y() - (screen_height_pt - 8.6)).abs() < 1e-6, "{sheet:?}");
    let glass = rounded_panels(&rig.last)
        .into_iter()
        .find(|(size, ..)| near(size.width, px(430.0), PX_TOLERANCE) && near(size.height, px(464.0), PX_TOLERANCE))
        .expect("поверхность шторки 430 × 464 pt");
    assert!(near(glass.1, px(54.0), PX_TOLERANCE), "радиус шторки {} px", glass.1);
    assert!(near(glass.2, px(sheet.center().y), PX_TOLERANCE));

    // Круги Ø 68: чёрные с α 0,5 [measured: C t=30.0]; центры x 91 / 220 / 349, центр первого ряда T + 83.
    let dark = circles(&rig.last, 68.0);
    assert_eq!(dark.len(), 7, "по кругу на каждый из 7 пунктов меню");
    for (_, _, fill) in &dark {
        assert_eq!((fill.r, fill.g, fill.b), (0.0, 0.0, 0.0), "круг чёрный, не светлый");
        assert!((fill.a - 0.5).abs() < 1e-6, "α круга {}", fill.a);
    }
    let first_row_y = px(sheet.min_y() + 83.0);
    let first_row: Vec<f32> = dark
        .iter()
        .filter(|(_, y, _)| near(*y, first_row_y, PX_TOLERANCE))
        .map(|(x, ..)| *x)
        .collect();
    assert_eq!(first_row.len(), 3, "в первом ряду три круга: {dark:?}");
    for expected_x in [91.0, 220.0, 349.0] {
        assert!(
            first_row.iter().any(|x| near(*x, px(expected_x), PX_TOLERANCE)),
            "центр x {expected_x} pt: {first_row:?}"
        );
    }
    let second_row_y = px(sheet.min_y() + 83.0 + 124.0);
    assert_eq!(
        dark.iter()
            .filter(|(_, y, _)| near(*y, second_row_y, PX_TOLERANCE))
            .count(),
        3,
        "второй ряд на 124 pt ниже"
    );

    // «Ручка» 37,6 × 3,7 pt, белая α 0,75, верх на 8,9 pt ниже верха шторки [measured: C t=30.0].
    let handle = rig
        .last
        .leaf_items()
        .find_map(|item| match item {
            DrawItem::Shape(shape) => match shape.geometry {
                ShapeGeometry::Capsule { size }
                    if near(size.width, px(37.6), 0.6) && near(size.height, px(3.7), 0.6) =>
                {
                    Some(shape.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .expect("ручка шторки");
    assert!((handle.fill.a - 0.75).abs() < 1e-6 && handle.fill.r == 1.0);
    assert!(near(handle.transform.tx, px(220.0), PX_TOLERANCE));
    assert!(near(
        handle.transform.ty,
        px(sheet.min_y() + 8.9 + 3.7 / 2.0),
        PX_TOLERANCE
    ));
}

#[test]
fn captions_are_uppercase_below_the_circle_and_only_the_active_icon_turns_yellow() {
    let mut rig = Rig::new();
    // Grid включаем настоящим выбором варианта, чтобы он стал «активным» в меню.
    enable_grid_and_return_to_menu(&mut rig);
    let yellow = UiKitTokens::default().colors.focus_yellow;
    let sheet = rig.rect_with(|g| g.sheet());
    for caption in ["FLASH", "ASPECT", "TIMER", "EXPOSURE", "RAW", "GRID", "LEVEL"] {
        assert_eq!(
            text_count_of(&rig.last, caption, TextRoleKind::ToolCaption),
            1,
            "подпись {caption} заглавными"
        );
    }
    // Базовая линия подписи — на 22,7 pt ниже низа круга [measured: R00 t=22.512]: у Flash (слот 0, первый ряд).
    let flash = texts_of(&rig.last, "FLASH", TextRoleKind::ToolCaption).remove(0);
    let circle_bottom_pt = sheet.min_y() + 83.0 + 34.0;
    assert!(
        near(flash.transform.ty, px(circle_bottom_pt + 22.7), PX_TOLERANCE),
        "базовая линия FLASH {} px",
        flash.transform.ty
    );
    assert_eq!(flash.color, UiKitTokens::default().colors.text_primary);
    // Все подписи белые α 0,88, не только активная: пик 228–231 на фоне 60 [measured: R00 t=22.512].
    assert!(
        (flash.opacity.get() - 0.88).abs() < 1e-3,
        "неактивная подпись белая α 0,88, а не {}",
        flash.opacity.get()
    );

    // Активный Grid: значок жёлтый, подпись белая α 0,88, круг по-прежнему чёрный [measured: R00 t=25.2].
    let grid_icon = icons_of(&rig.last, IconId::Grid);
    assert_eq!(grid_icon.len(), 1);
    assert_eq!(grid_icon[0].color, yellow, "значок активного инструмента жёлтый");
    let grid_caption = texts_of(&rig.last, "GRID", TextRoleKind::ToolCaption).remove(0);
    assert_eq!(
        grid_caption.color,
        UiKitTokens::default().colors.text_primary,
        "подпись не жёлтая"
    );
    assert!(
        (grid_caption.opacity.get() - 0.88).abs() < 0.01,
        "α подписи {}",
        grid_caption.opacity.get()
    );
    assert!(
        circles(&rig.last, 68.0)
            .iter()
            .all(|(_, _, fill)| fill.r == 0.0 && fill.g == 0.0)
    );
    // Неактивный инструмент (Timer) — белый значок.
    assert_eq!(
        icons_of(&rig.last, IconId::Timer)[0].color,
        UiKitTokens::default().colors.text_primary
    );
}

#[test]
fn sheet_appears_in_place_without_growing_out_of_the_menu_button() {
    let mut rig = Rig::new();
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    // Через 32 мс шторка уже имеет конечный размер (растворение на месте), а не размер кнопки меню.
    rig.run(32);
    let early = rounded_panels(&rig.last)
        .into_iter()
        .find(|(size, ..)| size.width > px(100.0))
        .expect("шторка уже в списке рисования");
    assert!(
        near(early.0.width, px(430.0), PX_TOLERANCE),
        "ширина {} px",
        early.0.width
    );
    assert!(
        near(early.0.height, px(464.0), PX_TOLERANCE),
        "высота {} px",
        early.0.height
    );
}

#[test]
fn ev_text_is_two_lines_white_at_zero_and_yellow_when_changed() {
    let mut rig = Rig::new();
    let tokens = UiKitTokens::default();
    rig.frame();
    // EV = 0: «0.0» над «EV», белые; значка солнца в ряду нет [measured: C t=18.0].
    assert_eq!(icons_of(&rig.last, IconId::Exposure).len(), 0);
    let value = texts_of(&rig.last, "0.0", TextRoleKind::StripValue).remove(0);
    let unit = texts_of(&rig.last, "EV", TextRoleKind::StripUnit).remove(0);
    assert_eq!(value.color, tokens.colors.text_primary);
    assert_eq!(unit.color, tokens.colors.text_primary);
    // Центры заглавных: число на y 98,5, «EV» на 111,6 (+ сдвиг к низу); базовая линия ниже центра на полкегля.
    let shift = bottom_shift_pt();
    let value_cap_px = value.run.cap_height_px;
    let unit_cap_px = unit.run.cap_height_px;
    assert!(near(
        value.transform.ty,
        px(98.5 + shift) + value_cap_px / 2.0,
        PX_TOLERANCE
    ));
    assert!(near(
        unit.transform.ty,
        px(111.6 + shift) + unit_cap_px / 2.0,
        PX_TOLERANCE
    ));
    // Оба текста стоят по центру одного слота (EV — третий из трёх значков, x = 263,6 pt).
    let center_x = |item: &ui_draw::draw_list::TextItem| item.transform.tx + item.run.advance_px / 2.0;
    assert!(near(center_x(&value), px(263.6), PX_TOLERANCE) && near(center_x(&unit), px(263.6), PX_TOLERANCE));

    // EV ≠ 0: оба жёлтые, число показывает значение.
    rig.act(ScreenAction::ExposureRestored {
        bias: camera_contract::control::ExposureBias::from_ev(2.0 / 3.0).expect("EV"),
    });
    rig.run(300);
    let changed_value = texts_of(&rig.last, "0.7", TextRoleKind::StripValue).remove(0);
    let changed_unit = texts_of(&rig.last, "EV", TextRoleKind::StripUnit).remove(0);
    assert_eq!(changed_value.color, tokens.colors.focus_yellow);
    assert_eq!(changed_unit.color, tokens.colors.focus_yellow);
}

#[test]
fn mode_pill_is_a_light_capsule_with_a_thin_border_inside_the_mode_capsule() {
    let mut rig = Rig::new();
    rig.frame();
    // Пилюля — светлая капсула с ободком; капсула режимов PHOTO тоже с ободком, но цвета поверхности (решение 297).
    let surface = ui_kit::tokens::UiKitTokens::default().surface.fill_color;
    let pill = rig
        .last
        .leaf_items()
        .find_map(|item| match item {
            DrawItem::Shape(shape)
                if shape.stroke.is_some()
                    && (shape.fill.r, shape.fill.g, shape.fill.b) != (surface.r, surface.g, surface.b) =>
            {
                match shape.geometry {
                    ShapeGeometry::Capsule { size } => Some((size, shape.clone())),
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("пилюля режима");
    // [measured: C t=18.0] 89 × 43 pt в центре капсулы (y 898,0 + сдвиг); белая заливка α 0,1, ободок 1 pt α 0,3.
    assert!(near(pill.0.width, px(89.0), PX_TOLERANCE) && near(pill.0.height, px(43.0), PX_TOLERANCE));
    assert!(near(pill.1.transform.ty, px(898.0 + bottom_shift_pt()), PX_TOLERANCE));
    assert!(near(pill.1.transform.tx, px(220.0), PX_TOLERANCE));
    assert!((pill.1.fill.a - 0.1).abs() < 1e-6);
    let stroke = pill.1.stroke.expect("ободок");
    assert!(near(stroke.width_px, px(1.0), 0.1) && (stroke.color.a - 0.3).abs() < 1e-6);
}

#[test]
fn grid_lines_are_one_point_wide_with_the_measured_opacity() {
    let mut rig = Rig::new();
    enable_grid_and_return_to_menu(&mut rig);
    rig.act(ScreenAction::ControlsMenuCloseRequested);
    rig.run(700);
    let frame = rig.rect_with(|g| g.frame(crate::settings::AspectRatio::FourByThree));
    // Рамка 4:3 — 159,3–746,0 pt эталона [measured: R00 t=10.027]; линии сетки — четыре тонкие полосы.
    assert!((frame.size.height - 586.7).abs() < 0.1, "{frame:?}");
    let lines: Vec<_> = rig
        .last
        .leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape) if (shape.fill.a - 0.36).abs() < 1e-6 => match shape.geometry {
                ShapeGeometry::RoundedRect { size, .. } if size.width.min(size.height) < px(2.0) => Some(size),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 4, "две вертикальные и две горизонтальные линии: {lines:?}");
    for size in lines {
        assert!(near(size.width.min(size.height), px(1.0), 0.1), "толщина {size:?}");
    }
}

#[test]
fn sheet_to_card_morph_starts_from_the_sheet_corner_radius() {
    let mut rig = Rig::new();
    open_menu(&mut rig);
    let timer_item = rig.center_with(|g| g.menu_item(2));
    rig.tap(timer_item);
    // Пока морф только начался, ни одна рамка не может быть скруглена как круглая кнопка (≈ 200+ pt):
    // шторка 54 pt и карточка, растущая из неё, 54 → 52 pt [measured: C t=37.8–38.1].
    rig.run(32);
    let early = rounded_panels(&rig.last);
    assert!(
        early.len() >= 2,
        "в кадре и шторка, и растущая из неё карточка: {early:?}"
    );
    for (size, radius, _) in early {
        assert!(radius <= px(60.0), "радиус {radius} px при рамке {size:?}");
    }
}

/// Центры y (px) круга затвора (кольцо Ø 80 pt) по кадрам до `until_ms` после действия.
fn shutter_y_track(rig: &mut Rig, until_ms: i64) -> Vec<f32> {
    let end = rig.now_ms + until_ms;
    let mut track = Vec::new();
    while rig.now_ms < end {
        rig.run(FRAME_MS);
        if let Some(y) = element_members(&rig.last).into_iter().find_map(|(geometry, _, y)| {
            matches!(geometry, ShapeGeometry::Circle { diameter } if near(diameter, px(80.0), 3.0)).then_some(y)
        }) {
            track.push(y);
        }
    }
    track
}

#[test]
fn shutter_rises_by_100_pt_in_about_a_third_of_a_second_without_overshoot() {
    let mut rig = Rig::new();
    rig.frame();
    let rest_y = shutter_y_track(&mut rig, 64).last().copied().expect("затвор в кадре");
    let ev_icon = rig.center_with(|g| g.strip_icon(2, 3));
    rig.tap(ev_icon);
    let track = shutter_y_track(&mut rig, 400);
    let raised_y = rest_y - px(100.0);
    // [measured: C t=54.0] поднят на 100 pt; пружина `CardMorph` (0,3 с, без перелёта): к 400 мс на месте.
    let last = *track.last().expect("кадры подъёма");
    assert!(
        near(last, raised_y, PX_TOLERANCE),
        "к 400 мс затвор на {last}, ждали {raised_y}"
    );
    // Без перелёта: затвор ни в один кадр не оказывается выше конечного места, и движение не идёт назад.
    assert!(track.iter().all(|y| *y >= raised_y - 0.5), "перелёт: {track:?}");
    assert!(
        track.windows(2).all(|pair| pair[1] <= pair[0] + 0.01),
        "возврат назад: {track:?}"
    );
}
