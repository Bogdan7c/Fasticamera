//! Белое в разделе калибровки пальцем через настоящие касания и кадры экрана (решения 289–294): капсула в полоске
//! прибора показывает Кельвины и открывает нижнюю карточку, в карточке шкала и «Phone» работают в глазных осях при
//! любом положении телефона, «Auto» нет, нижний ряд под карточкой скрыт и не ловит касаний, тап по завесе и смена
//! режима карточку закрывают.

use camera_contract::white::{WhiteAdjustment, WhiteOrigin, WhiteSourceId};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::{Point, Vector};
use ui_draw::icons::IconId;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, center, drag_eye};
use super::test_rig::*;
use super::touch_map_test_support::TouchMap;
use super::white_test_support::{camera_intents, facts_held, set_facts};
use crate::action::ScreenAction;
use crate::calibration::SectionCard;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::look_test_support::look_facts;
use crate::modes::CaptureMode;
use crate::white::WhiteAxis;
use crate::white_facts::WhiteFollowFacts;

const ORIENTATIONS: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

/// Раздел с ручным белым 4300 K.
fn section(orientation: DeviceOrientation) -> Rig {
    let mut rig = calibrating(look_facts(), orientation);
    set_facts(&mut rig, facts_held(4300.0, 0.0, WhiteOrigin::Manual));
    rig.effects();
    rig
}

fn capsule(rig: &Rig) -> Point {
    center(rig, "calib_white_capsule", 0)
}

/// Открыть карточку настоящим тапом по капсуле и дождаться конца проявления.
fn open_white_card(rig: &mut Rig) {
    let button = capsule(rig);
    rig.tap(button);
    rig.run(700);
}

/// Центры живых (ловящих касание) элементов раздела с подписью `label`.
fn live_centers(rig: &Rig, label: &str) -> Vec<Point> {
    let scene = rig.screen.scene_for_tests().expect("сцена построена");
    TouchMap::of_subtree(scene, "calibration_root")
        .live()
        .iter()
        .filter(|area| area.label == label)
        .filter_map(|area| area.effective.map(|rect| rect.center()))
        .collect()
}

/// Последнее белое, которое шкала отдала за жест (конец ведения).
fn kelvin_of(intents: &[ScreenCameraIntent]) -> Option<f64> {
    intents.iter().rev().find_map(|intent| match intent {
        ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Temperature(kelvin)) => Some(kelvin.kelvin()),
        _ => None,
    })
}

#[test]
fn the_capsule_shows_the_held_kelvin_without_an_auto_label_and_follows_the_facts() {
    for orientation in ORIENTATIONS {
        let mut rig = section(orientation);
        assert_eq!(
            text_count_of(&rig.last, "4300 K", TextRoleKind::Caption),
            1,
            "{orientation:?}: капсула показывает Кельвины, как число у затвора в PHOTO"
        );
        // Решение 294: в капсуле нет пометки «AUTO», даже когда слежение включено (оно на паузе).
        let mut facts = facts_held(5200.0, 0.0, WhiteOrigin::Source(WhiteSourceId::PHONE));
        facts.white.follow = WhiteFollowFacts::On;
        set_facts(&mut rig, facts);
        assert_eq!(
            text_count_of(&rig.last, "5200 K", TextRoleKind::Caption),
            1,
            "{orientation:?}"
        );
        assert_eq!(
            text_count_of(&rig.last, "AUTO", TextRoleKind::Caption),
            0,
            "{orientation:?}"
        );
        assert_eq!(
            text_count_of(&rig.last, "4300 K", TextRoleKind::Caption),
            0,
            "{orientation:?}: старое число ушло"
        );
    }
}

#[test]
fn a_tap_on_the_capsule_opens_the_white_card_and_a_second_tap_or_a_tap_on_the_scrim_closes_it() {
    for orientation in ORIENTATIONS {
        let mut rig = section(orientation);
        assert_eq!(rig.screen.state().calibration().card(), SectionCard::Closed);

        open_white_card(&mut rig);
        assert_eq!(
            rig.screen.state().calibration().card(),
            SectionCard::White,
            "{orientation:?}: тап открыл"
        );
        assert!(
            rig.effects().is_empty(),
            "{orientation:?}: открытие карточки не шлёт камере ничего"
        );

        // Повторный тап по капсуле: она лежит под модальной завесой, закрывает завеса.
        let button = capsule(&rig);
        rig.tap(button);
        rig.run(700);
        assert_eq!(
            rig.screen.state().calibration().card(),
            SectionCard::Closed,
            "{orientation:?}: повторный тап"
        );

        open_white_card(&mut rig);
        let free_place = center(&rig, "calib_knobs_window", 0);
        rig.tap(free_place);
        rig.run(700);
        assert_eq!(
            rig.screen.state().calibration().card(),
            SectionCard::Closed,
            "{orientation:?}: тап по завесе"
        );
        assert!(
            rig.effects().is_empty(),
            "{orientation:?}: закрытие намерений не порождает"
        );
    }
}

#[test]
fn the_card_has_the_axes_the_ruler_and_phone_with_the_lights_and_no_auto_chip() {
    for orientation in ORIENTATIONS {
        let mut rig = section(orientation);
        open_white_card(&mut rig);
        let lights = rig.screen.config.white.lights.len();

        assert_eq!(
            live_centers(&rig, "white_axis").len(),
            2,
            "{orientation:?}: числа Кельвинов и оттенка"
        );
        assert_eq!(live_centers(&rig, "white_ruler").len(), 1, "{orientation:?}: шкала");
        assert_eq!(
            live_centers(&rig, "white_light").len(),
            1 + lights,
            "{orientation:?}: «Phone» и свет, без «Auto»"
        );
        assert_eq!(
            text_count_of(&rig.last, "Phone", TextRoleKind::Caption),
            1,
            "{orientation:?}"
        );
        assert_eq!(
            text_count_of(&rig.last, "Auto", TextRoleKind::Caption),
            0,
            "{orientation:?}: фишки Auto нет (решение 294)"
        );
        assert_eq!(
            text_count_of(&rig.last, "4300 K", TextRoleKind::CardValue),
            1,
            "{orientation:?}: число оси"
        );
    }
}

#[test]
fn dragging_the_ruler_changes_the_kelvin_by_the_same_amount_whatever_the_phone_orientation() {
    let mut results = Vec::new();
    for orientation in ORIENTATIONS {
        let mut rig = section(orientation);
        open_white_card(&mut rig);
        let ruler = live_centers(&rig, "white_ruler")[0];
        drag_eye(&mut rig, ruler, Vector::new(-60.0, 0.0));
        let intents = camera_intents(&rig.effects());
        let kelvin =
            kelvin_of(&intents).unwrap_or_else(|| panic!("{orientation:?}: шкала не дала правки: {intents:?}"));
        assert!(
            !intents
                .iter()
                .any(|intent| matches!(intent, ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(_)))),
            "{orientation:?}: ось оттенка не тронута"
        );
        results.push((orientation, kelvin));
    }
    let (_, first) = results[0];
    assert!(
        (first - 4300.0).abs() > 50.0,
        "палец на 60 pt заметно двигает белое: {first}"
    );
    // Одно и то же глазное движение даёт одно и то же белое в любом положении телефона (допуск — отличие порога
    // распознавания пальца на разных поверхностях, не поворот осей: перепутанная ось дала бы 0 или обратный знак).
    let travel = (first - 4300.0).abs();
    for (orientation, kelvin) in &results {
        assert!(
            (kelvin - first).abs() < 0.1 * travel,
            "{orientation:?}: {kelvin} против {first} при ходе {travel}"
        );
    }
}

#[test]
fn selecting_the_tint_axis_sends_tint_edits_and_a_light_or_phone_tap_sends_their_intents() {
    for orientation in ORIENTATIONS {
        let mut rig = section(orientation);
        open_white_card(&mut rig);
        let axes = live_centers(&rig, "white_axis");
        // Порядок сборки: Кельвины, оттенок — на экране слева направо глазами; берём по состоянию после тапа.
        let tint_number = *axes
            .iter()
            .find(|point| **point != axes[0])
            .unwrap_or_else(|| panic!("{orientation:?}: две оси"));
        rig.tap(axes[1]);
        rig.run(200);
        assert_eq!(
            rig.screen.state().white().axis(),
            WhiteAxis::Tint,
            "{orientation:?}: тап выбрал ось оттенка"
        );
        let _ = tint_number;
        let ruler = live_centers(&rig, "white_ruler")[0];
        drag_eye(&mut rig, ruler, Vector::new(-60.0, 0.0));
        let intents = camera_intents(&rig.effects());
        assert!(
            intents
                .iter()
                .any(|intent| matches!(intent, ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(_)))),
            "{orientation:?}: {intents:?}"
        );

        let row = live_centers(&rig, "white_light");
        rig.tap(row[0]);
        let intents = camera_intents(&rig.effects());
        assert_eq!(
            intents,
            [ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE)],
            "{orientation:?}: первая кнопка ряда — «Phone»"
        );
        rig.tap(row[1]);
        let intents = camera_intents(&rig.effects());
        assert!(
            matches!(intents.as_slice(), [ScreenCameraIntent::SetWhite(_)]),
            "{orientation:?}: следующая — свет: {intents:?}"
        );
    }
}

#[test]
fn the_bottom_row_is_hidden_and_does_not_react_while_either_card_is_open() {
    for orientation in ORIENTATIONS {
        for card in [SectionCard::White, SectionCard::Presets] {
            let mut rig = section(orientation);
            assert!(
                text_count_of(&rig.last, "CLEAN", TextRoleKind::Caption) > 0,
                "{orientation:?}: ряд виден в покое"
            );
            assert!(!icons_of(&rig.last, IconId::ControlsMenu).is_empty(), "{orientation:?}");
            let clean = center(&rig, "calib_clean", 0);
            let menu_button = center(&rig, "menu_button", 0);
            let capsule_of_modes = center(&rig, "mode_capsule", 0);
            match card {
                SectionCard::White => open_white_card(&mut rig),
                _ => {
                    rig.act(ScreenAction::ControlsMenuOpenRequested);
                    rig.run(700);
                }
            }
            rig.effects();
            assert_eq!(rig.screen.state().calibration().card(), card);
            assert_eq!(
                text_count_of(&rig.last, "CLEAN", TextRoleKind::Caption),
                0,
                "{orientation:?} {card:?}: CLEAN под карточкой не рисуется"
            );
            assert!(
                icons_of(&rig.last, IconId::ControlsMenu).is_empty(),
                "{orientation:?} {card:?}: кнопка меню скрыта"
            );

            // Касание по месту каждого элемента ряда его не нажимает: точка лежит под карточкой или под завесой, а
            // ряд не получает ни нажатия, ни смены режима, ни нового меню.
            for (name, point) in [
                ("CLEAN", clean),
                ("кнопка меню", menu_button),
                ("капсула режимов", capsule_of_modes),
            ] {
                let before_card = rig.screen.state().calibration().card();
                rig.tap(point);
                rig.run(700);
                let state = rig.screen.state();
                assert_eq!(
                    state.mode(),
                    CaptureMode::Calibrate,
                    "{orientation:?} {card:?} {name}: режим тот же"
                );
                assert!(
                    matches!(state.calibration().card(), c if c == before_card || c == SectionCard::Closed),
                    "{orientation:?} {card:?} {name}: тап по ряду не открывает другую карточку"
                );
                assert_eq!(
                    state.calibration().clean_view(),
                    crate::calibration::CleanViewState::ShowingEdited,
                    "{orientation:?} {card:?} {name}: CLEAN не держится"
                );
                let closed = state.calibration().card() == SectionCard::Closed;
                let leftovers = rig.effects();
                // Точка лежит под самой карточкой: тап может попасть в её таблетку (это её дело), но «чистяк» не нажат.
                assert!(
                    !leftovers.iter().any(|effect| matches!(effect, Effect::CleanView(_))),
                    "{orientation:?} {card:?} {name}: «чистяк» нажат сквозь карточку: {leftovers:?}"
                );
                // К следующей точке карточка снова открыта тем же путём.
                if closed {
                    match card {
                        SectionCard::White => open_white_card(&mut rig),
                        _ => {
                            rig.act(ScreenAction::ControlsMenuOpenRequested);
                            rig.run(700);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_mode_change_closes_the_white_card_and_rotation_keeps_it_open() {
    let mut rig = section(DeviceOrientation::Portrait);
    open_white_card(&mut rig);
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(700);
    assert_eq!(
        rig.screen.state().calibration().card(),
        SectionCard::White,
        "поворот карточку не закрывает"
    );
    assert_eq!(
        live_centers(&rig, "white_ruler").len(),
        1,
        "после пересборки раздела карточка живая"
    );
    assert_eq!(text_count_of(&rig.last, "Auto", TextRoleKind::Caption), 0);

    rig.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    rig.run(700);
    assert_eq!(rig.screen.state().calibration().card(), SectionCard::Closed);
    assert!(!rig.screen.state().is_white_card_open());
}

#[test]
fn the_card_is_untouchable_while_closed() {
    let rig = section(DeviceOrientation::Portrait);
    for label in ["white_axis", "white_ruler", "white_light"] {
        assert!(
            live_centers(&rig, label).is_empty(),
            "{label}: закрытая карточка касаний не ловит"
        );
    }
}
