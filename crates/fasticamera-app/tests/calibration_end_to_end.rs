//! Сквозные тесты раздела калибровки на ПК (14b, решения 185–203): действие на экране → редьюсер → НАСТОЯЩИЙ
//! источник UI (`CameraScreenSource`) с исполнителем эффектов, библиотекой пресетов, фактами о виде и брокером
//! диалога → команда рендеру → НАСТОЯЩИЙ бэкенд показа (RADV, headless, полная validation). Доказательство —
//! ПИКСЕЛИ показанного видоискателя против эталона `isp-model` и состояние экрана (`screen.state()`), а не вызовы.
//!
//! Что доказывается:
//! * тяга ручки → правка в библиотеке → пиксели = эталон правленого вида; сброс ручки → пиксели = эталон пресета;
//!   факты на экране отличают ТОЛЬКО тронутую ручку;
//! * выбор основы ACES на экране → пиксели = эталон ACES;
//! * «чистяк»: пока зажат — пиксели = нейтраль на ТЕКУЩЕЙ основе, отпущен — правленый вид, библиотека не тронута;
//! * диалог имени: запрос доходит до платформы, ответ «Sunset» создаёт пресет, факты на экране его перечисляют;
//! * сбой записи файла → проблема в фактах → плашка на экране; вид при этом остаётся правленым;
//! * режим CALIBRATE просит у рендера прибор по раскладке (размер, взгляд по ориентации), выход его скрывает;
//!   свежий экран после переустановки источника просит прибор заново.
//!
//! Стенд — `calibration_rig` (общий для файла), стенд подставной камеры и эталона — файл `render-loop`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/viewfinder_rig/mod.rs"]
mod viewfinder_rig;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;

mod calibration_rig;

use calibration_rig::{AcceptingPersistence, CalibrationRig, FailingPersistence, library_over};
use camera_screen::modes::CaptureMode;
use camera_screen::{ProblemKind, ScreenAction};
use look_contract::{GlobalKnob, LookControl, LookKnob, LookProblemKind, LookSettings, PresetId, ToneBaseModel};
use render_loop::config::ToneBaseChoice;
use scope_contract::ScopeIntent;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::text_input::{TextInputOutcome, TextInputResult};
use viewfinder_rig::reddish;

/// Минимальное расхождение эталонов (в долях выхода), при котором тест различает «изменилось» от «нет». Допуск
/// сравнения пикселей — `viewfinder_rig::TOLERANCE`; расхождение эталонов должно быть заметно больше него.
const MIN_REFERENCE_GAP: f32 = 8.0 * viewfinder_rig::TOLERANCE;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("ручка в диапазоне")
}

/// Стенд с обычной библиотекой, экран уже в режиме CALIBRATE и показал первые кадры.
fn calibrating_rig() -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), reddish())?;
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(3);
    Some(rig)
}

/// Значения ручек активного пресета библиотеки — «как есть до правок».
fn preset_settings(rig: &CalibrationRig) -> LookSettings {
    rig.library.active_look().preset_recipe.settings
}

#[test]
fn dragging_a_knob_changes_the_shown_pixels_and_resetting_it_returns_the_preset_look_touching_only_that_knob() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let preset = preset_settings(&rig);
    rig.assert_shows(
        ToneBaseChoice::Agx,
        &preset,
        "после входа в CALIBRATE: вид активного пресета",
    );
    let facts = rig.screen().look_facts().expect("факты о виде дошли до экрана");
    assert!(
        !facts.control_differs(&LookControl::Knob(GlobalKnob::Saturation)),
        "правок ещё нет"
    );

    // Тяга ручки контраста: экран шлёт `SetLook` с одной заменённой ручкой, библиотека применяет, рендер показывает.
    rig.send(ScreenAction::KnobDragged {
        knob: GlobalKnob::Saturation,
        value: knob(0.9),
    });
    rig.frames(2);

    let edited = preset.clone().with_knob(GlobalKnob::Saturation, knob(0.9));
    assert_eq!(
        rig.library.active_settings(),
        edited,
        "библиотека держит правку ровно одной ручки"
    );
    assert!(
        rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Agx, &edited)) > MIN_REFERENCE_GAP,
        "эталоны правки и пресета различимы: тест что-то доказывает"
    );
    rig.assert_shows(ToneBaseChoice::Agx, &edited, "после тяги ручки: вид с правкой");
    let facts = rig.screen().look_facts().expect("факты");
    assert!(
        facts.control_differs(&LookControl::Knob(GlobalKnob::Saturation)),
        "тронутая ручка отличается"
    );
    for untouched in GlobalKnob::ALL
        .into_iter()
        .filter(|knob| *knob != GlobalKnob::Saturation)
    {
        assert!(
            !facts.control_differs(&LookControl::Knob(untouched)),
            "{untouched:?} не трогали"
        );
    }
    assert!(!facts.control_differs(&LookControl::ToneBase), "основу не трогали");

    // Сброс одной ручки возвращает значение ПРЕСЕТА, а не нуль (решение 196).
    rig.send(ScreenAction::ResetControlTapped(LookControl::Knob(
        GlobalKnob::Saturation,
    )));
    rig.frames(2);

    assert_eq!(rig.library.active_settings(), preset);
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "после сброса ручки: вид пресета");
    let facts = rig.screen().look_facts().expect("факты");
    assert!(
        !facts.control_differs(&LookControl::Knob(GlobalKnob::Saturation)),
        "ручка снова как у пресета"
    );
    rig.finish();
}

#[test]
fn selecting_the_aces_base_on_the_screen_shows_the_aces_picture() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let preset = preset_settings(&rig);
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "до смены основы: AgX");

    rig.send(ScreenAction::ToneBaseSelected(ToneBaseModel::Aces));
    rig.frames(2);

    assert_eq!(
        rig.library.active_recipe().base,
        ToneBaseModel::Aces,
        "библиотека держит новую основу"
    );
    assert!(
        rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Aces, &preset)) > MIN_REFERENCE_GAP,
        "картинки AgX и ACES различимы: тест что-то доказывает"
    );
    rig.assert_shows(ToneBaseChoice::Aces, &preset, "после выбора основы на экране: ACES");
    let facts = rig.screen().look_facts().expect("факты");
    assert!(
        facts.control_differs(&LookControl::ToneBase),
        "основа отличается от основы пресета"
    );
    assert_eq!(facts.current.base, ToneBaseModel::Aces);
    rig.finish();
}

#[test]
fn the_clean_view_shows_the_neutral_look_on_the_current_base_and_releasing_it_brings_the_edits_back() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    // Правка и другая основа: «чистяк» обязан взять нейтраль на ТЕКУЩЕЙ основе (ACES), а не на основе по умолчанию.
    rig.send(ScreenAction::ToneBaseSelected(ToneBaseModel::Aces));
    rig.send(ScreenAction::KnobDragged {
        knob: GlobalKnob::Warmth,
        value: knob(0.7),
    });
    rig.frames(2);
    let edited = rig.library.active_settings();
    rig.assert_shows(ToneBaseChoice::Aces, &edited, "до «чистяка»: правки на ACES");
    let library_before = rig.library.active_look();
    assert!(
        rig.reference_gap(
            (ToneBaseChoice::Aces, &edited),
            (ToneBaseChoice::Aces, &LookSettings::NEUTRAL)
        ) > MIN_REFERENCE_GAP,
        "правки видны: тест что-то доказывает"
    );

    rig.send(ScreenAction::CleanViewPressed);
    rig.frames(2);
    rig.assert_shows(
        ToneBaseChoice::Aces,
        &LookSettings::NEUTRAL,
        "«чистяк» зажат: нейтраль на ACES",
    );
    assert_eq!(
        rig.library.active_look(),
        library_before,
        "чистый вид библиотеку не трогает"
    );

    rig.send(ScreenAction::CleanViewReleased);
    rig.frames(2);
    rig.assert_shows(ToneBaseChoice::Aces, &edited, "«чистяк» отпущен: правки вернулись");
    assert_eq!(
        rig.library.active_look(),
        library_before,
        "и после отпускания библиотека та же"
    );
    rig.finish();
}

#[test]
fn the_name_dialog_creates_a_user_preset_and_the_screen_facts_list_it() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let presets_before = rig.screen().preset_entries().len();

    rig.send(ScreenAction::CreatePresetTapped);
    rig.frames(2);

    // Запрос дошёл до платформы диалога, а экран узнал номер своего запроса (обратная связь исполнителя).
    let shown = rig.dialogs.shown();
    assert_eq!(shown.len(), 1, "платформа получила ровно один запрос");
    assert_eq!(shown[0].purpose, shell_contract::text_input::TextPurpose::NewPresetName);
    assert_eq!(rig.broker.open_request_id(), Some(shown[0].id));

    // Платформа отвечает, как её ответ принимает маршрутизатор: брокер → приёмник → канал входа экрана.
    rig.broker.deliver_platform_result(TextInputResult {
        id: shown[0].id,
        outcome: TextInputOutcome::Entered("Sunset".to_owned()),
    });
    rig.frames(3);

    let active = rig.library.active_look();
    let created = rig
        .library
        .view_state()
        .presets
        .into_iter()
        .find(|entry| entry.name.as_str() == "Sunset")
        .expect("пресет Sunset создан в библиотеке");
    assert_eq!(active.preset, created.id, "новый пресет стал активным");
    let entries = rig.screen().preset_entries();
    assert_eq!(entries.len(), presets_before + 1, "экран видит на один пресет больше");
    assert!(
        entries
            .iter()
            .any(|entry| entry.name.as_str() == "Sunset" && entry.id == created.id)
    );
    assert_eq!(rig.screen().look_facts().expect("факты").active, created.id);
    assert!(matches!(created.id, PresetId::User(_)), "пресет свой, не встроенный");
    rig.finish();
}

#[test]
fn a_cancelled_name_dialog_changes_nothing_in_the_library() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let before = rig.library.view_state();

    rig.send(ScreenAction::CreatePresetTapped);
    rig.frames(2);
    let shown = rig.dialogs.shown();
    rig.broker.deliver_platform_result(TextInputResult {
        id: shown[0].id,
        outcome: TextInputOutcome::Cancelled,
    });
    rig.frames(3);

    assert_eq!(rig.library.view_state(), before, "отмена диалога библиотеку не меняет");
    rig.finish();
}

#[test]
fn a_failed_preset_file_write_raises_a_problem_banner_on_the_screen_and_keeps_the_edit_shown() {
    let Some(mut rig) = CalibrationRig::start(library_over(Box::new(FailingPersistence::disk_full())), reddish())
    else {
        return;
    };
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(3);
    assert!(rig.screen().look().banner().is_none(), "до правки плашки нет");

    rig.send(ScreenAction::KnobDragged {
        knob: GlobalKnob::Warmth,
        value: knob(0.5),
    });
    // Правка → просьба записи → итог «не записано» (опрос на начале кадра) → проблема в фактах → плашка.
    rig.frames(4);

    let banner = rig.screen().look().banner().expect("плашка проблемы поднята");
    assert!(
        matches!(banner.kind, ProblemKind::Look(LookProblemKind::SaveFailed(_))),
        "плашка про сбой записи: {banner:?}"
    );
    let facts = rig.screen().look_facts().expect("факты");
    assert!(facts.problem.is_some(), "проблема пришла в фактах");
    // Диск отказал, но правка живёт в памяти и видна: снимок и показ не зависят от записи файла.
    let edited = rig.library.active_settings();
    assert_eq!(edited.warmth, knob(0.5));
    rig.assert_shows(
        ToneBaseChoice::Agx,
        &edited,
        "правка видна, хотя записать её не удалось",
    );
    rig.finish();
}

#[test]
fn calibrate_mode_asks_the_render_for_a_scope_sized_by_the_layout_and_leaving_hides_it() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let layout = *rig
        .screen()
        .calibration()
        .layout()
        .expect("раскладка раздела построена");

    let ScopeIntent::Show(request) = *rig.scope_intents.last().expect("рендеру ушёл запрос прибора")
    else {
        panic!("ожидалось «показать»: {:?}", rig.scope_intents);
    };
    assert_eq!(
        request.image_size,
        layout.scope_image_size(),
        "размер — из раскладки раздела"
    );
    assert_eq!(request.scan, layout.scope_scan(), "взгляд — по ориентации телефона");
    assert_eq!(rig.scope_intents.len(), 1, "запрос один: повторов нет");

    // Телефон лёг набок: раскладка и взгляд меняются, рендер получает новый запрос.
    rig.send(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.frames(2);
    let rotated = *rig.screen().calibration().layout().expect("раскладка");
    let ScopeIntent::Show(rotated_request) = *rig.scope_intents.last().expect("запрос") else {
        panic!("ожидалось «показать»");
    };
    assert_eq!(rotated_request.scan, rotated.scope_scan());
    assert_ne!(
        rotated_request.scan, request.scan,
        "взгляд сменился вместе с ориентацией"
    );
    assert_eq!(rotated_request.image_size, rotated.scope_image_size());

    // Выход из режима скрывает прибор.
    rig.send(ScreenAction::ModeSelected(CaptureMode::Photo));
    rig.frames(2);
    assert_eq!(
        rig.scope_intents.last(),
        Some(&ScopeIntent::Hide),
        "выход из CALIBRATE скрывает прибор"
    );
    rig.finish();
}

#[test]
fn a_fresh_screen_after_the_ui_source_was_replaced_asks_the_render_for_the_scope_again() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    assert!(matches!(rig.scope_intents.last(), Some(ScopeIntent::Show(_))));
    assert!(
        rig.scope_requested_at_the_render().is_some(),
        "рендер держит запрос прибора"
    );
    let asked_before = rig.scope_intents.len();

    // Рендер ставит другой источник UI: запрос прибора отозван самим рендером, экран камеры собран заново
    // (переустановка ВСЕГДА даёт свежий экран в PHOTO: прежний экран вместе с источником уничтожен).
    rig.replace_screen_with_a_fresh_one();
    assert_eq!(
        rig.scope_requested_at_the_render(),
        None,
        "рендер сам отозвал запрос при замене источника: старый экран уничтожен вместе с ним, помнить о заказе некому"
    );
    rig.frames(2);
    assert_eq!(rig.screen().mode(), CaptureMode::Photo, "свежий экран начинает с PHOTO");
    assert_eq!(rig.scope_intents.len(), asked_before, "в PHOTO прибор не просят");

    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(3);
    assert_eq!(
        rig.scope_intents.len(),
        asked_before + 1,
        "свежий экран в CALIBRATE просит прибор заново"
    );
    assert!(matches!(rig.scope_intents.last(), Some(ScopeIntent::Show(_))));
    assert!(
        rig.scope_requested_at_the_render().is_some(),
        "рендер снова держит запрос прибора"
    );
    rig.finish();
}

#[test]
fn a_fresh_screen_releases_the_clean_view_the_previous_screen_held() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let preset = preset_settings(&rig);
    rig.send(ScreenAction::KnobDragged {
        knob: GlobalKnob::Saturation,
        value: knob(0.9),
    });
    rig.send(ScreenAction::CleanViewPressed);
    rig.frames(2);
    rig.assert_shows(
        ToneBaseChoice::Agx,
        &LookSettings::NEUTRAL,
        "«чистяк» зажат на старом экране",
    );

    // Старый экран исчез вместе с зажатой кнопкой; свежий экран обязан вернуть рендеру вид библиотеки.
    rig.replace_screen_with_a_fresh_one();
    rig.frames(3);

    let edited = preset.with_knob(GlobalKnob::Saturation, knob(0.9));
    rig.assert_shows(
        ToneBaseChoice::Agx,
        &edited,
        "свежий экран: рендер не остался на нейтрали",
    );
    rig.finish();
}
