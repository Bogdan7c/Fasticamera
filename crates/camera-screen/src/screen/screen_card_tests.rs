//! Нижняя карточка инструмента (решение 103) через настоящие касания: геометрия в px, потоки меню → карточка →
//! выбор → назад, значок над кадром → Exposure → линейка → «×», поднятый затвор, исчезновение нижнего ряда.

use ui_core::geometry::{Point, Rect};
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::settings::TimerSetting;
use crate::state::{PanelState, ShutterVisibility};
use crate::tools::ToolId;

/// Допуск сравнения координат, px.
const PX_TOLERANCE: f32 = 1.5;

/// Есть ли в списке видимая стеклянная форма с центром около точки `(x_pt; y_pt)` (pt экрана).
fn visible_glass_at(list: &DrawList, point: Point) -> bool {
    list.leaf_items().any(|item| match item {
        DrawItem::Glass(group) if group.opacity.get() > 0.01 => group
            .members
            .iter()
            .any(|member| near(member.transform.tx, px(point.x), 3.0) && near(member.transform.ty, px(point.y), 3.0)),
        _ => false,
    })
}

fn open_menu(rig: &mut Rig) {
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
}

/// Меню → пункт Timer (третий в сетке) настоящим тапом.
fn open_timer_card_from_menu(rig: &mut Rig) {
    open_menu(rig);
    // Порядок меню iOS 27: Flash, Aspect, Timer — Timer третий (слот 2).
    let timer_item = rig.center_with(|g| g.menu_item(2));
    rig.tap(timer_item);
    rig.run(700);
}

/// Значок EV над кадром (третий из трёх видимых) настоящим тапом.
fn open_exposure_card_from_icon(rig: &mut Rig) {
    let ev_icon = rig.center_with(|g| g.strip_icon(2, 3));
    rig.tap(ev_icon);
    rig.run(700);
}

fn shots(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Capture(_)))
        .count()
}

#[test]
fn card_frame_and_content_sit_at_the_measured_numbers_in_pixels() {
    let mut rig = Rig::new();
    open_exposure_card_from_icon(&mut rig);
    let card = rig.rect_with(|g| g.card());
    // Экран 1080 × 2412 px = 440 × 982,67 pt: карточка привязана к низу, отступ 8,2 pt [measured: C t=40.0].
    let screen_height_pt = f64::from(HEIGHT_PX) * 440.0 / f64::from(WIDTH_PX);
    assert!((card.max_y() - (screen_height_pt - 8.2)).abs() < 1e-3, "низ {card:?}");
    assert!((card.size.width - 421.0).abs() < 1e-9 && (card.size.height - 183.1).abs() < 1e-9);
    assert!((card.min_x() - 9.5).abs() < 1e-9 && (card.max_x() - 430.5).abs() < 1e-9);
    // Верх T = 764,7 + 26,67 (лишняя высота экрана 982,67 − 956 уходит вверх).
    let top = card.min_y();
    assert!((top - (764.7 + (screen_height_pt - 956.0))).abs() < 1e-3, "верх {top}");

    // Стекло в списке рисования стоит ровно там же, в пикселях.
    let glass = rig
        .last
        .leaf_items()
        .find_map(|item| match item {
            DrawItem::Glass(group) => group.members.iter().find_map(|member| match member.geometry {
                ShapeGeometry::RoundedRect { size, .. } if size.width > px(300.0) => Some((member.transform, size)),
                _ => None,
            }),
            _ => None,
        })
        .expect("стекло карточки");
    assert!(near(glass.1.width, px(421.0), PX_TOLERANCE) && near(glass.1.height, px(183.1), PX_TOLERANCE));
    assert!(near(glass.0.tx, px(220.0), PX_TOLERANCE));
    assert!(near(glass.0.ty, px(top + 183.1 / 2.0), PX_TOLERANCE));

    // Содержимое: центры от верха карточки T [measured: C t=40.0, t=54.0].
    let centre = |rect: Rect| (rect.center().x, rect.center().y - top);
    let (x, y) = centre(rig.rect_with(|g| g.card_left_button()));
    assert!((x - 49.0).abs() < 1e-6 && (y - 43.0).abs() < 1e-6);
    let right = rig.rect_with(|g| g.card_right_button());
    assert!((right.center().x - 391.0).abs() < 1e-6 && (right.center().y - top - 44.0).abs() < 1e-6);
    assert!((right.size.width - 36.0).abs() < 1e-9, "диск Ø 36");
    let title = rig.point_with(|g| g.card_title_center());
    let value = rig.point_with(|g| g.card_value_center());
    assert!((title.y - top - 30.0).abs() < 1e-6 && (value.y - top - 77.0).abs() < 1e-6);
    // Заголовок и значение действительно нарисованы на этих высотах.
    let drawn_y = |text: &str, role| {
        let expected = run_of(text, role);
        rig.last.leaf_items().find_map(|item| match item {
            DrawItem::Text(drawn) if drawn.run == expected => Some(drawn.transform.ty),
            _ => None,
        })
    };
    let title_y = drawn_y("EXPOSURE", TextRoleKind::CardTitle).expect("заголовок");
    let value_y = drawn_y("0.0", TextRoleKind::CardValue).expect("значение");
    // Базовая линия на половину высоты заглавных ниже центра, поэтому допуск в пол-кегля.
    assert!(near(title_y, px(title.y), px(8.0)) && near(value_y, px(value.y), px(14.0)));
    // Ряд вариантов из четырёх: центр на T+118, шаг 75,7, ряд по x = 220.
    let first = rig.rect_with(|g| g.card_option_slot(0, 4)).center();
    let last = rig.rect_with(|g| g.card_option_slot(3, 4)).center();
    assert!((first.y - top - 118.0).abs() < 1e-6);
    assert!((last.x - first.x - 3.0 * 75.7).abs() < 1e-6 && ((first.x + last.x) / 2.0 - 220.0).abs() < 1e-6);
    // Низ рисок шкалы — T+129.
    let ruler = rig.rect_with(|g| g.card_ruler());
    assert!((ruler.center().y + 13.0 / 2.0 - top - 129.0).abs() < 1e-6);
}

#[test]
fn menu_to_timer_card_selection_keeps_the_card_open_and_back_returns_to_the_menu() {
    let mut rig = Rig::new();
    open_timer_card_from_menu(&mut rig);
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Timer));
    // Карточка: заголовок, значение «Off», левая кнопка — «‹», не «×».
    assert_eq!(text_count_of(&rig.last, "TIMER", TextRoleKind::CardTitle), 1);
    assert_eq!(text_count_of(&rig.last, "Off", TextRoleKind::CardValue), 1);
    assert_eq!(icons_of(&rig.last, IconId::MenuChevron).len(), 1);
    assert!(icons_of(&rig.last, IconId::Close).is_empty());
    // Ряд: значок таймера и «3», «5», «10».
    assert_eq!(icons_of(&rig.last, IconId::Timer).len(), 1, "значок «выкл» в ряду");
    for label in ["3", "5", "10"] {
        assert_eq!(
            text_count_of(&rig.last, label, TextRoleKind::StripValue),
            1,
            "вариант {label}"
        );
    }

    // Тап по «5» (третий слот): настройка применена, карточка осталась и показывает «5s».
    let five = rig.center_with(|g| g.card_option_slot(2, 4));
    rig.tap(five);
    rig.run(100);
    assert_eq!(rig.screen.state().settings().timer, TimerSetting::Seconds5);
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Timer));
    assert_eq!(text_count_of(&rig.last, "5s", TextRoleKind::CardValue), 1);
    assert_eq!(text_count_of(&rig.last, "Off", TextRoleKind::CardValue), 0);
    // Выбранный вариант жёлтый, остальные белые.
    let colors = UiKitTokens::default().colors;
    let color_of = |label: &str| {
        let expected = run_of(label, TextRoleKind::StripValue);
        rig.last.leaf_items().find_map(|item| match item {
            DrawItem::Text(drawn) if drawn.run == expected => Some(drawn.color),
            _ => None,
        })
    };
    assert_eq!(color_of("5"), Some(colors.focus_yellow));
    assert_eq!(color_of("3"), Some(colors.text_primary));

    // «‹» ведёт обратно в меню.
    let back = rig.center_with(|g| g.card_left_button());
    rig.tap(back);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    rig.run(700);
    assert_eq!(
        text_count_of(&rig.last, "TIMER", TextRoleKind::CardTitle),
        0,
        "карточка ушла"
    );
    // Шторка снова целиком на месте: пункт Level нарисован иконкой.
    assert_eq!(icons_of(&rig.last, IconId::Level).len(), 1);
}

#[test]
fn strip_ev_icon_opens_the_exposure_card_whose_ruler_moves_ev_and_cross_closes_it() {
    let mut rig = Rig::new();
    open_exposure_card_from_icon(&mut rig);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    assert_eq!(text_count_of(&rig.last, "EXPOSURE", TextRoleKind::CardTitle), 1);
    assert_eq!(text_count_of(&rig.last, "0.0", TextRoleKind::CardValue), 1);
    assert_eq!(icons_of(&rig.last, IconId::Close).len(), 1, "со значка — «×»");
    assert!(icons_of(&rig.last, IconId::MenuChevron).is_empty());
    // «↻» есть всегда, но при EV = 0 приглушена.
    let reset = icons_of(&rig.last, IconId::Reset);
    assert_eq!(reset.len(), 1);
    assert!(
        near(reset[0].opacity.get(), 0.44, 0.01),
        "при нуле приглушена: {}",
        reset[0].opacity.get()
    );

    // Палец влево по линейке — EV растёт кратно трети; на кнопку «↻» ничего не уходит.
    rig.effects();
    let ruler = rig.center_with(|g| g.card_ruler());
    rig.drag(ruler, Point::new(ruler.x - 60.0, ruler.y), 320);
    rig.run(800);
    let steps = rig.screen.state().exposure().steps();
    assert!(steps > 0, "шагов {steps}");
    let sent: Vec<f32> = rig
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            Effect::Camera(ScreenCameraIntent::SetExposureBias(bias)) => Some(bias.ev()),
            _ => None,
        })
        .collect();
    assert!(!sent.is_empty());
    assert!(
        sent.iter().all(|ev| (ev * 3.0 - (ev * 3.0).round()).abs() < 1e-3),
        "не трети: {sent:?}"
    );
    // Значение в карточке — то, что в состоянии; «↻» теперь яркая.
    let ev = rig
        .screen
        .state()
        .exposure()
        .bias(&crate::ScreenConfig::default().exposure)
        .ev();
    let expected_text = super::super::widget::exposure_link::format_ev(ev);
    assert_eq!(
        text_count_of(&rig.last, &expected_text, TextRoleKind::CardValue),
        1,
        "{expected_text}"
    );
    assert!(near(icons_of(&rig.last, IconId::Reset)[0].opacity.get(), 1.0, 0.01));

    // «↻» сбрасывает EV в ноль и снова приглушается.
    let reset_button = rig.center_with(|g| g.card_right_button());
    rig.tap(reset_button);
    rig.run(300);
    assert_eq!(rig.screen.state().exposure().steps(), 0);
    assert!(near(icons_of(&rig.last, IconId::Reset)[0].opacity.get(), 0.44, 0.01));

    // «×» закрывает карточку (она открыта значком, меню за ней нет).
    let close = rig.center_with(|g| g.card_left_button());
    rig.tap(close);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
    rig.run(700);
    assert_eq!(text_count_of(&rig.last, "EXPOSURE", TextRoleKind::CardTitle), 0);
}

#[test]
fn reset_button_does_nothing_while_ev_is_zero() {
    let mut rig = Rig::new();
    open_exposure_card_from_icon(&mut rig);
    rig.effects();
    let reset_button = rig.center_with(|g| g.card_right_button());
    rig.tap(reset_button);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    assert!(
        !rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::Camera(ScreenCameraIntent::SetExposureBias(_)))),
        "нулевой EV сбрасывать нечего"
    );
}

#[test]
fn ruler_ticks_are_twenty_points_apart_and_whole_ev_ticks_are_brighter() {
    let mut rig = Rig::new();
    open_exposure_card_from_icon(&mut rig);
    let ruler = rig.rect_with(|g| g.card_ruler());
    let centre_x = px(ruler.center().x);
    let mut ticks: Vec<(f32, f32)> = rig
        .last
        .leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape) => match shape.geometry {
                ShapeGeometry::RoundedRect { size, .. }
                    if near(size.height, px(13.0), 0.6)
                        && near(size.width, px(2.0), 0.6)
                        && near(shape.transform.ty, px(ruler.center().y), 1.0) =>
                {
                    Some((shape.transform.tx, shape.fill.a * shape.opacity.get()))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    ticks.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!(ticks.len() >= 11, "в зоне затухания риски есть: {}", ticks.len());
    let pitch_px = px(20.0);
    for pair in ticks.windows(2) {
        assert!(near(pair[1].0 - pair[0].0, pitch_px, 0.5), "шаг рисок {:?}", pair);
    }
    for (x, alpha) in &ticks {
        let step = ((x - centre_x) / pitch_px).round() as i32;
        assert!(near(*x, centre_x + step as f32 * pitch_px, 0.5), "риска вне сетки: {x}");
        if step.abs() <= 4 {
            let expected = if step % 3 == 0 { 0.8 } else { 0.5 };
            assert!(
                near(*alpha, expected, 0.01),
                "шаг {step}: яркость {alpha}, ждали {expected}"
            );
        }
        assert!(step.abs() <= 6, "дальше ±6 шагов рисок нет: шаг {step}");
    }
    // Риска нулевого EV есть и она крупная.
    assert!(
        ticks
            .iter()
            .any(|(x, alpha)| near(*x, centre_x, 0.5) && near(*alpha, 0.8, 0.01))
    );
}

#[test]
fn the_raised_shutter_is_drawn_and_shoots_at_the_raised_place_while_a_card_is_open() {
    for from_menu in [false, true] {
        let mut rig = Rig::new();
        if from_menu {
            open_timer_card_from_menu(&mut rig);
        } else {
            open_exposure_card_from_icon(&mut rig);
        }
        assert_eq!(
            rig.screen.state().shutter_visibility(),
            ShutterVisibility::RaisedAboveCard
        );
        let raised = rig.rect_with(raised_shutter).center();
        let base = rig.shutter_center();
        // Поднят ровно на 100 pt: центр 801,9 − 100 = 701,9 (+ лишняя высота экрана).
        assert!((base.y - raised.y - 100.0).abs() < 1e-6);
        assert!(
            visible_glass_at(&rig.last, raised),
            "кольцо затвора нарисовано на поднятом месте (меню: {from_menu})"
        );

        // Тап на старом месте затвор не нажимает (там карточка), и панель остаётся открытой.
        rig.effects();
        let panel = rig.screen.state().panel();
        rig.tap(base);
        assert_eq!(shots(&rig.effects()), 0, "старое место не снимает (меню: {from_menu})");
        assert_eq!(rig.screen.state().panel(), panel);

        // Тап на поднятом месте снимает (таймер выключен у Exposure; у Timer-карточки он Off).
        rig.tap(raised);
        assert_eq!(shots(&rig.effects()), 1, "поднятый затвор снимает (меню: {from_menu})");
    }
}

#[test]
fn the_shutter_returns_to_its_place_after_the_card_closes() {
    let mut rig = Rig::new();
    open_exposure_card_from_icon(&mut rig);
    let base = rig.shutter_center();
    let close = rig.center_with(|g| g.card_left_button());
    rig.tap(close);
    rig.run(900);
    assert_eq!(rig.screen.state().shutter_visibility(), ShutterVisibility::Visible);
    assert!(visible_glass_at(&rig.last, base), "затвор вернулся на обычное место");
    rig.effects();
    rig.tap(base);
    assert_eq!(shots(&rig.effects()), 1, "на обычном месте снова снимает");
}

#[test]
fn thumbnail_capsule_and_menu_button_leave_while_a_card_is_open_and_return_after() {
    let mut rig = Rig::new();
    let thumbnail = rig.center_with(|g| g.thumbnail());
    let menu_button = rig.center_with(|g| g.menu_button());
    let capsule = rig.center_with(|g| g.capsule());
    for (name, point) in [
        ("миниатюра", thumbnail),
        ("кнопка меню", menu_button),
        ("капсула", capsule),
    ] {
        assert!(visible_glass_at(&rig.last, point), "{name} видна без карточки");
    }
    open_exposure_card_from_icon(&mut rig);
    for (name, point) in [
        ("миниатюра", thumbnail),
        ("кнопка меню", menu_button),
        ("капсула", capsule),
    ] {
        assert!(
            !visible_glass_at(&rig.last, point),
            "{name} растворилась, пока открыта карточка"
        );
    }
    // Тапы по их местам ничего не запускают: ни галереи, ни меню.
    rig.effects();
    rig.tap(thumbnail);
    rig.tap(menu_button);
    assert!(
        !rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::OpenSystemGallery))
    );
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);

    // Карточка закрыта — ряд вернулся и снова нажимается.
    let close = rig.center_with(|g| g.card_left_button());
    rig.tap(close);
    rig.run(1000);
    for (name, point) in [
        ("миниатюра", thumbnail),
        ("кнопка меню", menu_button),
        ("капсула", capsule),
    ] {
        assert!(visible_glass_at(&rig.last, point), "{name} вернулась");
    }
    rig.effects();
    rig.tap(thumbnail);
    assert!(
        rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::OpenSystemGallery))
    );
}

#[test]
fn tapping_the_viewfinder_closes_the_card_without_a_second_panel_left_behind() {
    let mut rig = Rig::new();
    open_timer_card_from_menu(&mut rig);
    let center = rig.frame_center();
    rig.tap(center);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed);
    rig.run(900);
    assert_eq!(text_count_of(&rig.last, "TIMER", TextRoleKind::CardTitle), 0);
    assert_eq!(icons_of(&rig.last, IconId::Level).len(), 0, "шторка тоже закрыта");
}

#[test]
fn re_entering_the_card_while_it_is_still_closing_keeps_it_tappable() {
    let mut rig = Rig::new();
    open_timer_card_from_menu(&mut rig);
    rig.act(ScreenAction::SubmenuBackRequested);
    rig.now_ms += 3 * FRAME_MS;
    rig.frame();
    rig.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Timer));
    let three = rig.center_with(|g| g.card_option_slot(1, 4));
    rig.tap(three);
    assert_eq!(
        rig.screen.state().settings().timer,
        TimerSetting::Seconds3,
        "карточка не «мёртвая»"
    );
}

#[test]
fn flash_card_offers_auto_on_off_and_taps_switch_the_flash_in_place() {
    use crate::settings::FlashMode;
    let mut rig = Rig::new();
    open_menu(&mut rig);
    let flash_item = rig.center_with(|g| g.menu_item(0));
    rig.tap(flash_item);
    rig.run(700);
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Flash));
    // Значок вспышки есть и над кадром; в карточке (ниже её верха) — ровно по одному значку на режим.
    let card_top_px = px(rig.rect_with(|g| g.card()).min_y());
    let in_card = |rig: &Rig, icon| {
        icons_of(&rig.last, icon)
            .into_iter()
            .filter(|drawn| drawn.transform.ty > card_top_px)
            .collect::<Vec<_>>()
    };
    assert_eq!(in_card(&rig, IconId::FlashAuto).len(), 1);
    assert_eq!(in_card(&rig, IconId::FlashOn).len(), 1);
    assert_eq!(in_card(&rig, IconId::FlashOff).len(), 1);
    // Порядок карточки iOS 27: слева направо Auto, On, Off.
    let x_of = |rig: &Rig, icon| in_card(rig, icon)[0].transform.tx;
    assert!(x_of(&rig, IconId::FlashAuto) < x_of(&rig, IconId::FlashOn));
    assert!(x_of(&rig, IconId::FlashOn) < x_of(&rig, IconId::FlashOff));
    assert_eq!(text_count_of(&rig.last, "Off", TextRoleKind::CardValue), 1);

    // Тап по каждому слоту применяет режим и обновляет крупное значение.
    for (index, mode, label) in [
        (0, FlashMode::Auto, "Auto"),
        (1, FlashMode::On, "On"),
        (2, FlashMode::Off, "Off"),
    ] {
        let slot = rig.center_with(|g| g.card_option_slot(index, 3));
        rig.tap(slot);
        assert_eq!(rig.screen.state().settings().flash, mode);
        rig.run(100);
        assert_eq!(text_count_of(&rig.last, label, TextRoleKind::CardValue), 1, "{label}");
    }
}
