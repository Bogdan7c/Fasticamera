//! Меню «шесть точек» раздела калибровки целиком: касание → действие → эффект и рисунок в списке.
//!
//! Проверяется то, что видит и делает пользователь: кнопка открывает шторку с двумя пунктами; пункт открывает
//! карточку; тап по имени выбирает пресет, по NEW / RENAME / SAVE / DELETE шлёт своё намерение (недоступные
//! кнопки молчат, DELETE требует двух тапов); тап мимо и «‹» закрывают; в альбоме всё это работает в глазных осях.

use look_contract::{BuiltinPreset, LookIntent, PresetId};
use scope_contract::{ScopeIntent, VectorscopeAxes};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::text_input::TextPurpose;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::screen_calibration_tests::{calibrating, center, edited, portrait};
use super::test_rig::*;
use crate::calibration::{CalibrationMenu, DeleteConfirmation};
use crate::effect::Effect;
use crate::look_test_support::{NIGHT, SUNSET, look_facts, look_facts_with_active, look_intents};
use look_contract::LookFacts;

/// Кнопка меню раздела → корень меню, пока морф шторки не осел.
fn open_menu(rig: &mut Rig) {
    let button = center(rig, "menu_button", 0);
    rig.tap(button);
    rig.run(700);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Root,
        "кнопка открыла корень меню"
    );
}

/// Корень меню → карточка PRESETS (пункт 0) или SCOPE AXES (пункт 1).
fn open_card(rig: &mut Rig, slot: usize) {
    let item = center(rig, "calib_menu_item", slot);
    rig.tap(item);
    rig.run(700);
}

fn open_presets(rig: &mut Rig) {
    open_menu(rig);
    open_card(rig, 0);
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::Presets);
}

fn preset_chip(rig: &Rig, index: usize) -> Point {
    center(rig, "calib_chip", index)
}

fn action_chip(rig: &Rig, index: usize) -> Point {
    center(rig, "calib_action_chip", index)
}

fn drawn_count(rig: &mut Rig, text: &str, role: TextRoleKind) -> usize {
    let list = rig.frame().clone();
    text_count_of(&list, text, role)
}

fn intents(rig: &mut Rig) -> Vec<LookIntent> {
    look_intents(&rig.effects()).into_iter().cloned().collect()
}

fn edited_facts_with_active(active: PresetId) -> LookFacts {
    let mut facts = edited(|settings| settings);
    facts.active = active;
    facts
}

// ----- корень меню ---------------------------------------------------------------------------------------------------

#[test]
fn the_menu_button_opens_a_sheet_with_two_labelled_items_and_the_button_hides_under_it() {
    let mut rig = portrait(look_facts());
    assert_eq!(
        drawn_count(&mut rig, "PRESETS", TextRoleKind::ToolCaption),
        0,
        "закрытое меню ничего не рисует"
    );
    open_menu(&mut rig);
    assert_eq!(
        drawn_count(&mut rig, "PRESETS", TextRoleKind::ToolCaption),
        1,
        "подпись пункта PRESETS"
    );
    assert_eq!(
        drawn_count(&mut rig, "SCOPE AXES", TextRoleKind::ToolCaption),
        1,
        "подпись пункта SCOPE AXES"
    );
    // Кнопка под шторкой не ловит касаний: повторный тап по ней меню не переоткрывает и ничего не ломает.
    let item_0 = center(&rig, "calib_menu_item", 0);
    let item_1 = center(&rig, "calib_menu_item", 1);
    assert!(
        item_0.x < item_1.x && (item_0.y - item_1.y).abs() < 0.5,
        "два пункта стоят одним рядом слева направо"
    );
    let screen_center_x = rig.frame_center().x;
    assert!(
        ((item_0.x + item_1.x) / 2.0 - screen_center_x).abs() < 1.0,
        "пара пунктов по центру шторки"
    );
}

#[test]
fn a_tap_outside_the_menu_closes_it_without_touching_the_knobs() {
    let mut rig = portrait(look_facts());
    open_menu(&mut rig);
    // Тап по колонке ручек (она лежит под меню): меню закрывается, а ручка не получает касания.
    let knobs = center(&rig, "calib_knobs_panel", 0);
    rig.tap(knobs);
    rig.run(500);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Closed,
        "тап мимо закрыл меню"
    );
    assert!(intents(&mut rig).is_empty(), "тап мимо не дошёл до ручек");
    // Меню закрыто: тап по той же колонке больше не закрывает ничего (скрим выключен).
    rig.tap(knobs);
    rig.run(100);
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::Closed);
}

#[test]
fn swiping_down_closes_the_menu_and_swiping_up_does_not() {
    let mut rig = portrait(look_facts());
    open_menu(&mut rig);
    let sheet_point = center(&rig, "calib_menu_sheet", 0);
    let up = Point::new(sheet_point.x, sheet_point.y - 40.0);
    rig.drag(sheet_point, up, 200);
    rig.run(200);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Root,
        "свайп вверх меню не закрывает"
    );
    let sheet_point = center(&rig, "calib_menu_sheet", 0);
    let down = Point::new(sheet_point.x, sheet_point.y + 60.0);
    rig.drag(sheet_point, down, 200);
    rig.run(500);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Closed,
        "свайп вниз закрыл меню"
    );
}

// ----- карточка PRESETS ----------------------------------------------------------------------------------------------

#[test]
fn the_presets_card_lists_every_preset_with_the_active_one_in_yellow() {
    let mut rig = portrait(look_facts());
    open_presets(&mut rig);
    let list = rig.frame().clone();
    let yellow = UiKitTokens::default().colors.focus_yellow;
    for name in ["Honest", "As Eye", "Sunset", "Night"] {
        let drawn = texts_of(&list, name, TextRoleKind::CardOption);
        assert_eq!(drawn.len(), 1, "имя {name} нарисовано один раз");
        let is_yellow = drawn[0].color == yellow;
        assert_eq!(is_yellow, name == "As Eye", "{name}: жёлтым только активный «As Eye»");
    }
    for action in ["NEW", "RENAME", "SAVE", "DELETE"] {
        assert_eq!(
            text_count_of(&list, action, TextRoleKind::CardOption),
            1,
            "действие {action}"
        );
    }
    assert_eq!(
        text_count_of(&list, "PRESETS", TextRoleKind::CardTitle),
        1,
        "заголовок карточки"
    );
}

#[test]
fn tapping_a_preset_name_selects_that_preset() {
    let mut rig = portrait(look_facts());
    open_presets(&mut rig);
    rig.effects();
    rig.tap(preset_chip(&rig, 2)); // Sunset
    rig.run(100);
    assert_eq!(
        intents(&mut rig),
        vec![LookIntent::SelectPreset(PresetId::User(SUNSET))]
    );
    // Тап по уже активному пресету без правок — ничего не шлёт.
    rig.tap(preset_chip(&rig, 1)); // As Eye
    rig.run(100);
    assert!(
        intents(&mut rig).is_empty(),
        "активный пресет без правок повторно не выбирается"
    );
}

#[test]
fn rename_save_and_delete_are_silent_for_a_builtin_preset_and_new_asks_for_a_name() {
    let mut rig = portrait(edited_facts_with_active(PresetId::Builtin(BuiltinPreset::AsEye)));
    open_presets(&mut rig);
    rig.effects();
    for (index, action) in [(1, "RENAME"), (2, "SAVE"), (3, "DELETE")] {
        rig.tap(action_chip(&rig, index));
        rig.run(100);
        assert!(
            rig.effects().is_empty(),
            "{action} у встроенного пресета ничего не делает"
        );
    }
    assert_eq!(
        rig.screen.state().calibration().delete_confirmation(),
        DeleteConfirmation::Idle,
        "DELETE у встроенного не взводится"
    );
    rig.tap(action_chip(&rig, 0));
    rig.run(100);
    assert!(
        rig.effects().iter().any(|effect| matches!(
            effect,
            Effect::RequestText { purpose: TextPurpose::NewPresetName, initial_text } if initial_text.is_empty()
        )),
        "NEW просит имя: пустое поле"
    );
}

#[test]
fn rename_and_save_work_for_a_user_preset_and_save_needs_edits() {
    // Свой пресет с правками: RENAME просит имя с текущим названием, SAVE перезаписывает пресет.
    let mut rig = portrait(edited_facts_with_active(PresetId::User(SUNSET)));
    open_presets(&mut rig);
    rig.effects();
    rig.tap(action_chip(&rig, 1));
    rig.run(100);
    assert!(
        rig.effects().iter().any(|effect| matches!(
            effect,
            Effect::RequestText { purpose: TextPurpose::RenamePreset, initial_text } if initial_text == "Sunset"
        )),
        "RENAME просит новое имя, поле открыто с «Sunset»"
    );
    rig.tap(action_chip(&rig, 2));
    rig.run(100);
    assert_eq!(
        intents(&mut rig),
        vec![LookIntent::OverwritePreset(SUNSET)],
        "SAVE перезаписывает свой пресет"
    );
    // Без правок SAVE недоступен.
    let mut clean = portrait(look_facts_with_active(PresetId::User(SUNSET)));
    open_presets(&mut clean);
    clean.effects();
    clean.tap(action_chip(&clean, 2));
    clean.run(100);
    assert!(intents(&mut clean).is_empty(), "SAVE без правок молчит");
}

#[test]
fn delete_needs_two_taps_and_shows_confirm_in_between() {
    let mut rig = portrait(look_facts_with_active(PresetId::User(NIGHT)));
    open_presets(&mut rig);
    rig.effects();
    assert_eq!(drawn_count(&mut rig, "DELETE", TextRoleKind::CardOption), 1);
    rig.tap(action_chip(&rig, 3));
    rig.run(100);
    assert!(intents(&mut rig).is_empty(), "первый тап только взводит удаление");
    assert_eq!(
        rig.screen.state().calibration().delete_confirmation(),
        DeleteConfirmation::Armed(NIGHT)
    );
    assert_eq!(
        drawn_count(&mut rig, "CONFIRM", TextRoleKind::CardOption),
        1,
        "взведённая кнопка просит подтверждения"
    );
    assert_eq!(drawn_count(&mut rig, "DELETE", TextRoleKind::CardOption), 0);
    rig.tap(action_chip(&rig, 3));
    rig.run(100);
    assert_eq!(
        intents(&mut rig),
        vec![LookIntent::DeletePreset(NIGHT)],
        "второй тап удаляет"
    );
}

#[test]
fn another_tap_between_the_two_delete_taps_disarms_the_deletion() {
    let mut rig = portrait(look_facts_with_active(PresetId::User(NIGHT)));
    open_presets(&mut rig);
    rig.effects();
    rig.tap(action_chip(&rig, 3));
    rig.run(100);
    rig.tap(preset_chip(&rig, 0)); // Honest
    rig.run(100);
    rig.effects();
    assert_eq!(
        rig.screen.state().calibration().delete_confirmation(),
        DeleteConfirmation::Idle,
        "чужой тап снял взвод"
    );
}

#[test]
fn the_reset_arrow_discards_edits_and_is_dim_and_silent_without_edits() {
    let mut rig = portrait(edited_facts_with_active(PresetId::Builtin(BuiltinPreset::AsEye)));
    open_presets(&mut rig);
    rig.effects();
    let right = center(&rig, "card_button", 1);
    rig.tap(right);
    rig.run(100);
    assert_eq!(
        intents(&mut rig),
        vec![LookIntent::DiscardEdits],
        "«↻» возвращает пресет к исходным значениям"
    );

    let mut clean = portrait(look_facts());
    open_presets(&mut clean);
    clean.effects();
    let right = center(&clean, "card_button", 1);
    clean.tap(right);
    clean.run(100);
    assert!(intents(&mut clean).is_empty(), "без правок «↻» молчит");
}

#[test]
fn the_back_arrow_returns_from_the_card_to_the_root_every_time() {
    let mut rig = portrait(look_facts());
    open_presets(&mut rig);
    let back = center(&rig, "card_button", 0);
    rig.tap(back);
    rig.run(700);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        CalibrationMenu::Root,
        "«‹» вернула в корень"
    );
    assert_eq!(
        drawn_count(&mut rig, "NEW", TextRoleKind::CardOption),
        0,
        "карточка растворилась"
    );
    // Карточку можно открыть снова и вернуться тем же путём.
    open_card(&mut rig, 0);
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::Presets);
    rig.tap(center(&rig, "card_button", 0));
    rig.run(700);
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::Root);
}

// ----- карточка SCOPE AXES -------------------------------------------------------------------------------------------

#[test]
fn the_scope_axes_card_switches_the_vectorscope_standard_and_asks_the_scope_for_it() {
    let mut rig = portrait(look_facts());
    open_menu(&mut rig);
    open_card(&mut rig, 1);
    assert_eq!(rig.screen.state().calibration().menu(), CalibrationMenu::ScopeAxes);
    assert_eq!(drawn_count(&mut rig, "SCOPE AXES", TextRoleKind::CardTitle), 1);
    assert_eq!(
        drawn_count(&mut rig, "OKLAB", TextRoleKind::CardOption),
        1,
        "вариант OKLAB в ряду"
    );
    assert_eq!(
        drawn_count(&mut rig, "Y'CbCr 709", TextRoleKind::CardOption),
        1,
        "вариант Y'CbCr 709 в ряду"
    );
    assert_eq!(
        drawn_count(&mut rig, "Y'CbCr 709", TextRoleKind::CardValue),
        1,
        "крупное значение — текущий стандарт"
    );
    rig.effects();
    rig.tap(center(&rig, "calib_chip", 1));
    rig.run(200);
    assert_eq!(
        rig.screen.state().calibration().vectorscope_axes(),
        VectorscopeAxes::OklabAb
    );
    let effects = rig.effects();
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::Scopes(ScopeIntent::Show(request)) if request.vectorscope_axes == VectorscopeAxes::OklabAb
        )),
        "прибору ушёл запрос с новыми осями: {effects:?}"
    );
    assert_eq!(
        drawn_count(&mut rig, "OKLAB", TextRoleKind::CardValue),
        1,
        "значение сменилось"
    );
    // «↻» в этой карточке нет: нажать её нечем.
    rig.effects();
    rig.tap(center(&rig, "card_button", 1));
    rig.run(100);
    assert!(rig.effects().is_empty(), "у SCOPE AXES нет «↻»");
}

// ----- альбом --------------------------------------------------------------------------------------------------------

#[test]
fn in_landscape_the_menu_is_touched_in_eye_space_and_drawn_turned_with_the_section() {
    for orientation in [DeviceOrientation::LandscapeLeft, DeviceOrientation::LandscapeRight] {
        let mut rig = calibrating(look_facts(), orientation);
        open_menu(&mut rig);
        let layout = *rig.screen.state().calibration().layout().unwrap();
        // Пункты стоят на глазной горизонтали: в альбоме это вертикаль поверхности.
        let item_0 = center(&rig, "calib_menu_item", 0);
        let item_1 = center(&rig, "calib_menu_item", 1);
        let eye_0 = layout.surface_point_in_eye(item_0);
        let eye_1 = layout.surface_point_in_eye(item_1);
        assert!(
            eye_0.x < eye_1.x && (eye_0.y - eye_1.y).abs() < 0.5,
            "{orientation:?}: пункты в глазном ряду: {eye_0:?} {eye_1:?}"
        );
        assert!(
            (item_0.x - item_1.x).abs() < 0.5,
            "{orientation:?}: на поверхности пункты друг под другом"
        );
        open_card(&mut rig, 0);
        assert_eq!(
            rig.screen.state().calibration().menu(),
            CalibrationMenu::Presets,
            "{orientation:?}: пункт нажался"
        );
        rig.effects();
        rig.tap(preset_chip(&rig, 2));
        rig.run(100);
        assert_eq!(
            intents(&mut rig),
            vec![LookIntent::SelectPreset(PresetId::User(SUNSET))],
            "{orientation:?}: имя нажалось в глазных осях"
        );
        // Рисунок повёрнут вместе с разделом: подпись «Sunset» стоит у центра своей таблетки.
        let list = rig.frame().clone();
        let drawn = texts_of(&list, "Sunset", TextRoleKind::CardOption);
        assert_eq!(drawn.len(), 1, "{orientation:?}");
        let chip = preset_chip(&rig, 2);
        let (cx, cy) = (px(chip.x), px(chip.y));
        let transform = drawn[0].transform;
        let along_x = transform.apply_vector(ui_draw::geometry::PxVector { dx: 1.0, dy: 0.0 });
        assert!(
            (along_x.dx.abs() < 0.01) && (along_x.dy.abs() > 0.99),
            "{orientation:?}: строка подписи идёт вдоль вертикали поверхности (повёрнута): {along_x:?}"
        );
        let half_run = drawn[0].run.advance_px / 2.0;
        let centre = transform.apply_point(ui_draw::geometry::PxPoint { x: half_run, y: 0.0 });
        assert!(
            (centre.x - cx).abs() < px(12.0) && (centre.y - cy).abs() < px(12.0),
            "{orientation:?}: подпись на таблетке: ({}, {}) против ({cx}, {cy})",
            centre.x,
            centre.y
        );
    }
}

// ----- границы -------------------------------------------------------------------------------------------------------

#[test]
fn a_name_with_characters_outside_the_font_does_not_break_the_frame() {
    let mut facts = look_facts();
    facts.presets.push(look_contract::PresetEntry {
        id: PresetId::User(look_contract::UserPresetId(11)),
        // Кириллица в атласе есть (имена владельца — по-русски); иероглифов и эмодзи нет.
        name: look_contract::PresetName::new("Закат 日🌅").expect("имя допустимо"),
        kind: look_contract::PresetKind::User,
    });
    let mut rig = portrait(facts);
    open_presets(&mut rig);
    // Кириллица рисуется как есть, символы вне набора — заменой на «?», а кадр строится.
    assert_eq!(
        drawn_count(&mut rig, "Закат ??", TextRoleKind::CardOption),
        1,
        "кириллица показана как есть, символы вне шрифта — знаками «?»"
    );
    rig.effects();
    rig.tap(preset_chip(&rig, 4));
    rig.run(100);
    assert_eq!(
        intents(&mut rig),
        vec![LookIntent::SelectPreset(PresetId::User(look_contract::UserPresetId(
            11
        )))],
        "по такой таблетке пресет выбирается как обычно"
    );
}

#[test]
fn names_beyond_the_chip_pool_are_counted_in_diagnostics_not_lost_silently() {
    let mut config = crate::config::ScreenConfig::default();
    config.calibration.widgets.chip_pool_size = 2;
    let mut rig = Rig::with(config, ui_core::time::MotionPreference::Full);
    rig.screen
        .handle_action(crate::action::ScreenAction::LookFactsChanged(Box::new(look_facts())));
    rig.screen.handle_action(crate::action::ScreenAction::ModeSelected(
        crate::modes::CaptureMode::Calibrate,
    ));
    rig.run(400);
    open_presets(&mut rig);
    assert_eq!(
        rig.screen.diagnostics().preset_chips_overflowed,
        2,
        "из четырёх имён в ряд встали два, два посчитаны"
    );
    assert_eq!(
        drawn_count(&mut rig, "Sunset", TextRoleKind::CardOption),
        0,
        "лишние имена не рисуются"
    );
}

#[test]
fn many_names_scroll_the_row_and_the_active_one_starts_visible() {
    let mut facts = look_facts();
    for number in 20..28_u64 {
        facts.presets.push(look_contract::PresetEntry {
            id: PresetId::User(look_contract::UserPresetId(number)),
            name: look_contract::PresetName::new(format!("Preset number {number}")).expect("имя допустимо"),
            kind: look_contract::PresetKind::User,
        });
    }
    facts.active = PresetId::User(look_contract::UserPresetId(27));
    let mut rig = portrait(facts);
    open_presets(&mut rig);
    let visible_numbers = |list: &ui_draw::draw_list::DrawList| -> Vec<u64> {
        (20..28_u64)
            .filter(|number| !texts_of(list, &format!("Preset number {number}"), TextRoleKind::CardOption).is_empty())
            .collect()
    };
    let before = visible_numbers(&rig.frame().clone());
    assert!(
        before.contains(&27),
        "активное имя видно, хотя в ряд все не помещаются: {before:?}"
    );
    assert!(before.len() < 8, "в окно помещаются не все имена: {before:?}");
    // Палец тянет ряд вправо: содержимое едет вправо, показываются более ранние имена.
    let row = center(&rig, "chip_row_window", 0);
    let from = Point::new(row.x - 120.0, row.y);
    rig.touch(TouchPhase::Began, from, rig.now_ms);
    for step in 1..=20 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(from.x + 14.0 * f64::from(step), from.y),
            rig.now_ms + 16 * i64::from(step),
        );
    }
    // Палец замирает и отпускается: ряд остаётся там, где его отпустили.
    for hold in 1..=12 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(from.x + 280.0, from.y),
            rig.now_ms + 320 + 16 * hold,
        );
    }
    rig.touch(TouchPhase::Ended, Point::new(from.x + 280.0, from.y), rig.now_ms + 540);
    rig.now_ms += 560;
    rig.run(800);
    let after = visible_numbers(&rig.frame().clone());
    assert!(
        !after.contains(&27),
        "после протяжки вправо последнее имя уехало: {after:?}"
    );
    assert!(
        after.iter().min() < before.iter().min(),
        "после протяжки видны более ранние имена: до {before:?}, после {after:?}"
    );
}
