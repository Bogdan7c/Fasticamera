//! Кнопка белого у затвора (WB35): где стоит, как едет вместе с затвором, когда видна и нажимается. Всё через
//! настоящие кадры и касания экрана: проверяются нарисованное стекло и итог касаний, а не внутренние поля.

use camera_contract::white::WhiteSourceId;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Rect};
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawList;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::UiKitTokens;

use super::test_rig::*;
use super::white_test_support::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::{CameraFacts, CameraPresence, UnavailableReason};
use crate::modes::CaptureMode;
use crate::settings::TimerSetting;
use crate::state::PanelState;
use crate::tools::{ToolId, ToolOption};
use crate::white_config::WhiteButtonSide;

const WHITE_CARD: PanelState = PanelState::Submenu(ToolId::White);
/// Диаметр кнопки меню и кнопки белого, pt.
const BUTTON_DIAMETER_PT: f64 = 44.0;

fn config_with_side(side: WhiteButtonSide) -> ScreenConfig {
    let mut config = ScreenConfig::default();
    config.white.button_side = side;
    config
}

/// Нарисованный круг кнопки белого: (центр x, центр y) в px. Кнопку меню (тот же диаметр) отличает x.
fn white_circle(list: &DrawList, center_x_pt: f64) -> Option<(f32, f32)> {
    glass_members(list)
        .into_iter()
        .find_map(|(geometry, x, y)| match geometry {
            ShapeGeometry::Circle { diameter }
                if (diameter - px(BUTTON_DIAMETER_PT)).abs() < 1.0 && (x - px(center_x_pt)).abs() < 1.0 =>
            {
                Some((x, y))
            }
            _ => None,
        })
}

/// Кольцо затвора: центр y в px.
fn shutter_ring_y(list: &DrawList) -> f32 {
    glass_members(list)
        .into_iter()
        .find_map(|(geometry, _, y)| match geometry {
            ShapeGeometry::Circle { diameter } if diameter > px(60.0) => Some(y),
            _ => None,
        })
        .expect("кольцо затвора")
}

// ---- где стоит ----

#[test]
fn the_right_button_is_a_44_pt_glass_circle_at_the_shutter_height_between_shutter_and_menu_button() {
    let mut rig = Rig::new();
    rig.run(300);
    let shutter = rig.shutter_center();
    let (x, y) = white_circle(&rig.last, shutter.x + 92.0).expect("стеклянный круг кнопки белого");
    assert!(near(x, px(shutter.x + 92.0), 0.5));
    assert!(near(y, px(shutter.y), 0.5), "на высоте центра затвора");
    // Между краем затвора и кнопкой меню, с зазорами с обеих сторон.
    let menu = rig.rect_with(|g| g.menu_button());
    let button = targets(&rig).button;
    assert!(button.min_x() > shutter.x + UiKitTokens::default().shutter.outer_diameter_pt / 2.0 + 8.0);
    assert!(button.max_x() < menu.min_x() - 8.0);
    assert!(
        (button.size.width - menu.size.width).abs() < 1e-9,
        "диаметр как у кнопки меню"
    );
}

#[test]
fn the_left_side_mirrors_the_button_around_the_shutter() {
    let mut rig = Rig::with(config_with_side(WhiteButtonSide::Left), MotionPreference::Full);
    rig.run(300);
    let shutter = rig.shutter_center();
    let (x, _) = white_circle(&rig.last, shutter.x - 92.0).expect("круг слева");
    assert!(near(x, px(shutter.x - 92.0), 0.5));
    assert!(white_circle(&rig.last, shutter.x + 92.0).is_none(), "справа круга нет");
    assert!((targets(&rig).button.center().x - (shutter.x - 92.0)).abs() < 1e-9);
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.min_x() < b.max_x() && b.min_x() < a.max_x() && a.min_y() < b.max_y() && b.min_y() < a.max_y()
}

#[test]
fn at_rest_the_button_overlaps_no_neighbour_on_either_side() {
    for side in [WhiteButtonSide::Right, WhiteButtonSide::Left] {
        let mut rig = Rig::with(config_with_side(side), MotionPreference::Full);
        rig.run(300);
        let button = targets(&rig).button;
        let neighbours = [
            ("миниатюра", rig.rect_with(|g| g.thumbnail())),
            ("капсула режимов", rig.rect_with(|g| g.capsule())),
            ("кнопка меню", rig.rect_with(|g| g.menu_button())),
            ("лента зума", rig.rect_with(|g| g.zoom_strip())),
            ("переворот камеры", rig.rect_with(|g| g.flip_button())),
            ("затвор", rig.rect_with(|g| g.shutter())),
        ];
        for (name, rect) in neighbours {
            assert!(
                !intersects(button, rect),
                "{side:?}: кнопка налезает на «{name}»: {button:?} / {rect:?}"
            );
        }
    }
}

#[test]
fn raised_over_a_card_the_button_stays_clear_of_the_raised_shutter_on_either_side() {
    for side in [WhiteButtonSide::Right, WhiteButtonSide::Left] {
        let mut rig = Rig::with(config_with_side(side), MotionPreference::Full);
        open_card(&mut rig);
        let button = targets(&rig).button;
        let shutter = rig.rect_with(|g| g.shutter());
        let raised_shutter = Rect::from_xywh(
            shutter.min_x(),
            shutter.min_y() - 124.0,
            shutter.size.width,
            shutter.size.height,
        );
        assert!(
            !intersects(button, raised_shutter),
            "{side:?}: {button:?} / {raised_shutter:?}"
        );
    }
}

// ---- едет вместе с затвором ----

/// Расстояние между центром кнопки белого и кольцом затвора по вертикали в каждом кадре анимации, px.
fn distances_while(rig: &mut Rig, side_x_pt: f64, frames: usize) -> Vec<f32> {
    let mut distances = Vec::new();
    for _ in 0..frames {
        rig.run(FRAME_MS);
        let (_, button_y) = white_circle(&rig.last, side_x_pt).expect("кнопка в кадре");
        distances.push(button_y - shutter_ring_y(&rig.last));
    }
    distances
}

#[test]
fn the_button_rises_and_falls_with_the_shutter_at_a_constant_distance_in_every_frame() {
    for side in [WhiteButtonSide::Right, WhiteButtonSide::Left] {
        for motion in [MotionPreference::Full, MotionPreference::Reduced] {
            let mut rig = Rig::with(config_with_side(side), motion);
            rig.run(300);
            let shutter = rig.shutter_center();
            let x = match side {
                WhiteButtonSide::Right => shutter.x + 92.0,
                WhiteButtonSide::Left => shutter.x - 92.0,
            };
            let rest_distance = {
                let (_, y) = white_circle(&rig.last, x).expect("кнопка");
                y - shutter_ring_y(&rig.last)
            };
            let start_ring = shutter_ring_y(&rig.last);
            // Подъём над белой карточкой (124 pt), переключение на EV (100 pt), закрытие (0).
            for (label, action) in [
                ("открыли белую", ScreenAction::WhiteButtonTapped),
                ("переключили на EV", ScreenAction::StripToolTapped(ToolId::Exposure)),
                ("закрыли", ScreenAction::ExposurePanelCloseRequested),
            ] {
                rig.act(action);
                let distances = distances_while(&mut rig, x, 60);
                for (frame, distance) in distances.iter().enumerate() {
                    assert!(
                        (distance - rest_distance).abs() < 0.01,
                        "{side:?} {motion:?} «{label}», кадр {frame}: расстояние {distance} вместо {rest_distance}"
                    );
                }
            }
            // Сам затвор при этом действительно ездил (иначе постоянное расстояние ничего не доказывает).
            assert!(
                (shutter_ring_y(&rig.last) - start_ring).abs() < 0.5,
                "вернулись на место"
            );
        }
    }
}

#[test]
fn over_the_white_card_the_button_stands_124_pt_higher_and_over_the_others_100_pt() {
    let mut rig = Rig::new();
    rig.run(300);
    let rest = targets(&rig).button;
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(900);
    assert!(((rest.min_y() - targets(&rig).button.min_y()) - 100.0).abs() < 1e-9);
    rig.act(ScreenAction::WhiteButtonTapped);
    rig.run(900);
    assert!(((rest.min_y() - targets(&rig).button.min_y()) - 124.0).abs() < 1e-9);
    // Нарисованный круг стоит там же, где цель касания.
    let (_, y) = white_circle(&rig.last, rest.center().x).expect("кнопка");
    assert!(near(y, px(targets(&rig).button.center().y), 0.5));
}

#[test]
fn the_raised_button_stays_fully_above_the_system_gesture_band_and_below_the_card() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let button = targets(&rig).button;
    let screen_height_pt = rig.screen.state().layout().expect("раскладка").screen_size().height;
    let band_top = screen_height_pt - rig.screen.config.widgets.white_card.system_gesture_band_pt;
    assert!(
        button.max_y() <= band_top,
        "зона кнопки {button:?} заходит в полосу жеста {band_top}"
    );
    let card_top = rig.rect_with(|g| g.white_card()).min_y();
    assert!(button.max_y() <= card_top, "кнопка стоит над карточкой, а не на ней");
    // В покое — тоже выше полосы.
    rig.act(ScreenAction::WhiteButtonTapped);
    rig.run(900);
    assert!(targets(&rig).button.max_y() <= band_top);
}

// ---- остаётся рабочей на поднятом месте ----

#[test]
fn the_raised_button_closes_the_open_card_with_one_tap_and_switches_from_another_card() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let raised = white_button(&rig);
    rig.tap(raised);
    assert_eq!(
        rig.screen.state().panel(),
        PanelState::Closed,
        "тап по поднятой кнопке закрывает"
    );
    rig.run(700);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed, "и не открывает заново");
    // С карточки EV — переключает на белую.
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(700);
    let raised = white_button(&rig);
    rig.tap(raised);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
}

#[test]
fn the_raised_button_takes_the_phone_white_on_a_long_press_and_the_card_stays() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let raised = white_button(&rig);
    long_press(&mut rig, raised, 700);
    let takes = camera_intents(&rig.effects())
        .into_iter()
        .filter(|intent| *intent == ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE))
        .count();
    assert_eq!(takes, 1);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
}

// ---- когда видна и нажимается ----

#[test]
fn under_the_controls_drawer_the_button_dissolves_and_cannot_be_pressed() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
    assert!(
        white_circle(&rig.last, button.x).is_none(),
        "стекла кнопки под шторкой нет"
    );
    rig.effects();
    long_press(&mut rig, button, 700);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    assert!(
        camera_intents(&rig.effects())
            .iter()
            .all(|intent| !matches!(intent, ScreenCameraIntent::TakeWhite(_))),
        "белое под шторкой не берётся"
    );
}

#[test]
fn in_calibrate_the_button_is_hidden_and_does_not_react() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.run(900);
    assert!(
        white_circle(&rig.last, button.x).is_none(),
        "в CALIBRATE кнопки нет в кадре"
    );
    rig.effects();
    let rejections_before = rig.screen.state().rejections().total();
    rig.tap(button);
    long_press(&mut rig, button, 700);
    // Кнопка не просто отвергнута охраной режима: сам узел не ловит касания, действие даже не рождается.
    assert_eq!(
        rig.screen.state().rejections().total(),
        rejections_before,
        "касание дошло до редьюсера"
    );
    assert_ne!(rig.screen.state().panel(), WHITE_CARD);
    assert!(
        camera_intents(&rig.effects())
            .iter()
            .all(|intent| !matches!(intent, ScreenCameraIntent::TakeWhite(_)))
    );
}

#[test]
fn the_button_behaves_like_the_menu_button_during_the_countdown_and_with_no_camera() {
    // Отсчёт таймера: кнопка на месте и нажимается, как кнопка меню.
    let mut rig = Rig::new();
    let button = white_button(&rig);
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
    rig.act(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds10,
    )));
    rig.act(ScreenAction::ControlsMenuCloseRequested);
    rig.run(700);
    rig.act(ScreenAction::ShutterTapped { at: ms(rig.now_ms) });
    rig.run(300);
    assert!(matches!(
        rig.screen.state().shutter(),
        crate::shutter::ShutterState::Countdown { .. }
    ));
    assert!(
        white_circle(&rig.last, button.x).is_some(),
        "во время отсчёта кнопка видна"
    );
    rig.tap(button);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD, "и открывает карточку");

    // Камеры нет: кнопка видна и нажимается (решение «камеры нет» принимает движок), белое просит у движка.
    let mut rig = Rig::new();
    rig.act(ScreenAction::CameraFactsChanged(CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: UnavailableReason::InUse,
        },
        ..CameraFacts::STARTING
    }));
    rig.run(400);
    assert!(white_circle(&rig.last, button.x).is_some(), "без камеры кнопка видна");
    rig.effects();
    long_press(&mut rig, button, 700);
    assert!(
        camera_intents(&rig.effects())
            .iter()
            .any(|intent| matches!(intent, ScreenCameraIntent::TakeWhite(_)))
    );
}

#[test]
fn a_tap_on_the_button_never_reaches_the_viewfinder_underneath() {
    let mut rig = Rig::new();
    let button = white_button(&rig);
    rig.tap(button);
    let effects = rig.effects();
    assert!(
        effects.iter().all(|effect| !matches!(effect, Effect::Camera(_))),
        "под кнопкой фокус не наводится: {effects:?}"
    );
    // Тап по видоискателю закрыл бы открывшуюся карточку: она на месте — значит сработала одна кнопка.
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
}

/// Итог «палец 1 держит затвор, палец 2 тапает `second`»: открытая панель и эффекты, потом оба пальца подняты.
fn two_finger_outcome(second_target: impl Fn(&Rig) -> Point) -> (PanelState, Vec<Effect>) {
    let mut rig = Rig::new();
    let shutter = rig.shutter_center();
    let second = second_target(&rig);
    rig.effects();
    let start = rig.now_ms;
    rig.touch_pointer(1, TouchPhase::Began, shutter, start);
    rig.touch_pointer(2, TouchPhase::Began, second, start + 20);
    rig.touch_pointer(2, TouchPhase::Ended, second, start + 60);
    rig.now_ms = start + 80;
    rig.frame();
    rig.touch_pointer(1, TouchPhase::Ended, shutter, rig.now_ms);
    rig.now_ms += 60;
    rig.frame();
    let outcome = (rig.screen.state().panel(), rig.effects());
    // Пальцы подняты: ничего не залипло, обычные касания работают.
    rig.run(700);
    let button = white_button(&rig);
    rig.tap(button);
    assert_eq!(
        rig.screen.state().panel(),
        WHITE_CARD,
        "после двух пальцев кнопка снова работает"
    );
    outcome
}

#[test]
fn a_second_finger_on_the_white_button_behaves_exactly_like_one_on_the_menu_button() {
    // Арена жестов отменяет однопальцевые жесты, когда лёг второй палец (`ExtraFingers::FailOrCancel`, behavior.md
    // 1.2): затвор под первым пальцем так же не снимает при втором на кнопке меню. Кнопка белого не должна вести
    // себя иначе: не брать белое, не открывать панель и не ломать затвор.
    let (white_panel, white_effects) = two_finger_outcome(white_button);
    let (menu_panel, menu_effects) = two_finger_outcome(|rig| rig.center_with(|g| g.menu_button()));
    assert_eq!(white_panel, menu_panel);
    assert_eq!(white_effects, menu_effects);
    assert!(
        camera_intents(&white_effects)
            .iter()
            .all(|intent| !matches!(intent, ScreenCameraIntent::TakeWhite(_)))
    );
}
