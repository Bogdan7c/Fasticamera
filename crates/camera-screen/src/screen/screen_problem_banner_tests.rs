//! Плашка проблемы пресетов: появляется на новую проблему (в PHOTO и в разделе калибровки), сама гаснет по сроку
//! из конфига и показывается заново на новую проблему, даже одинаковую.

use look_contract::{LookFacts, LookProblem, LookProblemKind, PresetError, PresetNameError, StorageFailure};
use shell_contract::orientation::DeviceOrientation;
use ui_draw::draw_list::DrawItem;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, zone_px};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::look_test_support::look_facts;
use crate::look_view::ProblemKind;
use crate::widget::problem_banner::problem_text;

/// Факты с проблемой библиотеки номер `serial`.
fn problem_facts(serial: u64, kind: LookProblemKind) -> LookFacts {
    LookFacts {
        problem: Some(LookProblem { serial, kind }),
        ..look_facts()
    }
}

fn seconds() -> i64 {
    (crate::config::ScreenConfig::default()
        .calibration
        .widgets
        .problem_banner_seconds
        * 1000.0) as i64
}

fn shown(rig: &mut Rig, text: &str) -> usize {
    let list = rig.frame().clone();
    text_count_of(&list, text, TextRoleKind::BannerText)
}

fn push(rig: &mut Rig, facts: LookFacts) {
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(facts)));
}

#[test]
fn every_problem_kind_has_its_own_short_english_text() {
    let cases = [
        (
            ProblemKind::Look(LookProblemKind::Rejected(PresetError::NameTaken)),
            "NAME TAKEN",
        ),
        (
            ProblemKind::Look(LookProblemKind::Rejected(PresetError::BuiltinIsReadOnly(
                look_contract::BuiltinPreset::Honest,
            ))),
            "READ ONLY",
        ),
        (
            ProblemKind::Look(LookProblemKind::SaveFailed(StorageFailure::WriteFailed(
                "диск".to_owned(),
            ))),
            "SAVE FAILED",
        ),
        (ProblemKind::Look(LookProblemKind::SavesRefused), "SAVES OFF"),
        (ProblemKind::InvalidPresetName(PresetNameError::Empty), "INVALID NAME"),
        (ProblemKind::KeyboardUnavailable, "KEYBOARD UNAVAILABLE"),
    ];
    for (kind, text) in cases {
        assert_eq!(problem_text(&kind), text, "{kind:?}");
    }
}

#[test]
fn a_new_problem_shows_the_plaque_in_photo_and_it_goes_away_by_itself() {
    let mut rig = Rig::new();
    push(
        &mut rig,
        problem_facts(1, LookProblemKind::Rejected(PresetError::NameTaken)),
    );
    rig.run(300);
    assert_eq!(shown(&mut rig, "NAME TAKEN"), 1, "плашка нарисована");
    assert!(rig.screen.state().look().banner().is_some());
    rig.run(seconds() + 800);
    assert!(
        rig.screen.state().look().banner().is_none(),
        "по сроку экран получил ProblemDismissed"
    );
    assert_eq!(shown(&mut rig, "NAME TAKEN"), 0, "плашка погасла");
}

#[test]
fn an_identical_problem_with_a_new_serial_is_shown_again_and_restarts_the_timer() {
    let mut rig = Rig::new();
    push(
        &mut rig,
        problem_facts(1, LookProblemKind::Rejected(PresetError::NameTaken)),
    );
    rig.run(seconds() * 2 / 3);
    // Второй раз то же самое, но с новым номером: плашка не гаснет в срок первой, срок идёт с нуля.
    push(
        &mut rig,
        problem_facts(2, LookProblemKind::Rejected(PresetError::NameTaken)),
    );
    rig.run(seconds() * 2 / 3);
    assert!(
        rig.screen
            .state()
            .look()
            .banner()
            .is_some_and(|banner| banner.number == 2),
        "ещё висит вторая плашка (первая уже отжила бы свой срок)"
    );
    assert_eq!(shown(&mut rig, "NAME TAKEN"), 1);
    rig.run(seconds() + 800);
    assert!(
        rig.screen.state().look().banner().is_none(),
        "вторая тоже погасла по своему сроку"
    );
    // Те же факты ещё раз (тот же номер) плашку не воскрешают.
    push(
        &mut rig,
        problem_facts(2, LookProblemKind::Rejected(PresetError::NameTaken)),
    );
    rig.run(300);
    assert_eq!(shown(&mut rig, "NAME TAKEN"), 0, "тот же номер не показывается дважды");
}

#[test]
fn in_calibrate_the_plaque_stands_at_the_bottom_row_outside_the_clean_preview_in_every_orientation() {
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
    ] {
        let mut rig = calibrating(look_facts(), orientation);
        // Локальная проблема экрана: клавиатуры нет.
        rig.screen.handle_action(ScreenAction::CreatePresetTapped);
        rig.screen.handle_action(ScreenAction::TextRequestNotSent);
        rig.run(400);
        let list = rig.frame().clone();
        let drawn = texts_of(&list, "KEYBOARD UNAVAILABLE", TextRoleKind::BannerText);
        assert_eq!(drawn.len(), 1, "{orientation:?}: плашка видна в разделе");
        let layout = *rig.screen.state().calibration().layout().unwrap();
        // Плашка не заходит в чистую зону превью (решение 203).
        let (left, top, right, bottom) = zone_px(&rig, layout.zones().clean_preview);
        let item = DrawItem::Text(drawn[0].clone());
        let bounds = item.bounds_px().expect("границы подписи плашки");
        let overlaps =
            bounds.origin.x < right && bounds.max_x() > left && bounds.origin.y < bottom && bounds.max_y() > top;
        assert!(!overlaps, "{orientation:?}: плашка в чистой зоне: {bounds:?}");
        // И стоит над нижним рядом (в глазных осях): центр плашки выше верха нижнего ряда.
        let centre = ui_core::geometry::Point::new(
            f64::from(bounds.origin.x + bounds.size.width / 2.0) / f64::from(PX_PER_PT),
            f64::from(bounds.origin.y + bounds.size.height / 2.0) / f64::from(PX_PER_PT),
        );
        let eye = layout.surface_point_in_eye(centre);
        assert!(
            eye.y < layout.zones().bottom_row.min_y(),
            "{orientation:?}: плашка над нижним рядом: {eye:?} против {:?}",
            layout.zones().bottom_row
        );
    }
}

#[test]
fn in_photo_the_plaque_moves_below_the_lock_plaque_instead_of_overlapping_it() {
    let mut rig = Rig::new();
    push(&mut rig, problem_facts(1, LookProblemKind::SavesRefused));
    rig.run(300);
    let list = rig.frame().clone();
    let alone = texts_of(&list, "SAVES OFF", TextRoleKind::BannerText);
    assert_eq!(alone.len(), 1);
    let alone_y = alone[0].transform.ty;
    // Включаем AE/AF LOCK: проблема встаёт ниже плашки LOCK.
    let center = rig.frame_center();
    rig.screen.handle_action(ScreenAction::ViewfinderLongPressed {
        point: center,
        at: ms(rig.now_ms),
    });
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(crate::CameraFacts {
            lock: camera_contract::three_a::AeAfLock::Locked,
            ..crate::test_support::facts_with_point(
                camera_contract::three_a::SensorPoint::new(0.5, 0.5).expect("центр"),
                1,
            )
        }));
    rig.run(800);
    assert!(
        rig.screen.state().focus().is_lock_plaque_visible(),
        "плашка LOCK показана"
    );
    let list = rig.frame().clone();
    let with_lock = texts_of(&list, "SAVES OFF", TextRoleKind::BannerText);
    assert_eq!(with_lock.len(), 1);
    assert!(
        with_lock[0].transform.ty > alone_y + 10.0,
        "ниже плашки LOCK: {} против {alone_y}",
        with_lock[0].transform.ty
    );
}

#[test]
fn with_the_presets_card_open_the_plaque_stands_above_it_in_every_orientation() {
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
    ] {
        let mut rig = calibrating(look_facts(), orientation);
        // Проблемы поднимает в основном сама карточка PRESETS (NEW, RENAME, SAVE): плашка не должна лечь на неё.
        rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
        rig.screen.handle_action(ScreenAction::CalibrationMenuItemTapped(
            crate::calibration::CalibrationMenuItem::Presets,
        ));
        rig.run(900);
        rig.screen.handle_action(ScreenAction::CreatePresetTapped);
        rig.screen.handle_action(ScreenAction::TextRequestNotSent);
        rig.run(500);
        let list = rig.frame().clone();
        let drawn = texts_of(&list, "KEYBOARD UNAVAILABLE", TextRoleKind::BannerText);
        assert_eq!(drawn.len(), 1, "{orientation:?}: плашка видна при открытой карточке");
        let bounds = DrawItem::Text(drawn[0].clone())
            .bounds_px()
            .expect("границы подписи плашки");
        let layout = *rig.screen.state().calibration().layout().unwrap();
        // Низ плашки в глазных осях: он обязан быть выше верха карточки.
        let corners = [
            (bounds.origin.x, bounds.origin.y),
            (bounds.max_x(), bounds.origin.y),
            (bounds.origin.x, bounds.max_y()),
            (bounds.max_x(), bounds.max_y()),
        ];
        let lowest_eye_y = corners
            .iter()
            .map(|(x, y)| {
                layout
                    .surface_point_in_eye(ui_core::geometry::Point::new(
                        f64::from(*x) / f64::from(PX_PER_PT),
                        f64::from(*y) / f64::from(PX_PER_PT),
                    ))
                    .y
            })
            .fold(f64::MIN, f64::max);
        let card = crate::config::ScreenConfig::default().widgets.card;
        let card_top = layout.eye_size().height - card.bottom_inset_pt - card.height_pt;
        assert!(
            lowest_eye_y < card_top,
            "{orientation:?}: плашка (низ {lowest_eye_y} pt) легла на карточку (верх {card_top} pt)"
        );
    }
}
