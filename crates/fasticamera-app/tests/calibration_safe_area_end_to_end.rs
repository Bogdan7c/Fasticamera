//! Безопасная зона и палец насквозь (14k, замечания 2 и 7 владельца, решение 226): палец в пикселях поверхности с
//! полосами телефона → канал входа → экран → библиотека пресетов → команда рендеру → ПИКСЕЛИ показанного кадра.
//!
//! Пара тестов на одном и том же стенде: без безопасной зоны (как до 14k) палец по AGX уходит системе и картинка не
//! меняется; с зоной тот же жест по карусели на её новом месте показывает кадр моделью ACES. Числа — телефон владельца.

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

#[allow(
    dead_code,
    reason = "стенд общий: каждый тестовый файл берёт своё подмножество шагов"
)]
mod calibration_rig;

use calibration_rig::{AcceptingPersistence, CalibrationRig, library_over};
use camera_screen::ScreenAction;
use camera_screen::modes::CaptureMode;
use render_loop::config::ToneBaseChoice;
use shell_contract::edge_insets::RawEdgeInsetsPx;
use shell_contract::safe_area::SafeAreaInsets;
use shell_contract::touch::TouchPhase;
use viewfinder_rig::reddish;

/// Полосы обязательных системных жестов телефона владельца (`dumpsys window`, 24.11.2026), px.
const OWNER_INSETS: RawEdgeInsetsPx = RawEdgeInsetsPx {
    left: 0,
    top: 156,
    right: 0,
    bottom: 132,
};
/// Палец ведут по карусели: от `SWIPE_FROM_X` до `SWIPE_TO_X`, px.
const SWIPE_FROM_X: f32 = 235.0;
const SWIPE_TO_X: f32 = 45.0;
/// Центр шапки с y = 0 (как до 14k): отступ панели 12 pt + половина шапки 28 pt, 1080 / 440 px на pt.
const OLD_HEADER_Y: f32 = 98.0;
/// Центр шапки под полосой 156 px.
const SAFE_HEADER_Y: f32 = OLD_HEADER_Y + 156.0;
const SWIPE_STEPS: usize = 10;

/// Раздел открыт, телефон сообщил полосы жестов и `safe_area`.
fn calibrating_on_the_owner_phone(safe_area: SafeAreaInsets) -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), reddish())?;
    rig.report_screen_edges(OWNER_INSETS, safe_area);
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(3);
    Some(rig)
}

/// Медленный свайп влево на высоте `y_px` с задержкой в конце (выбор решает положение, а не скорость отрыва).
fn swipe_left_at(rig: &mut CalibrationRig, y_px: f32) {
    rig.touch(TouchPhase::Began, SWIPE_FROM_X, y_px);
    rig.frame();
    for step in 1..=SWIPE_STEPS {
        let x = SWIPE_FROM_X + (SWIPE_TO_X - SWIPE_FROM_X) * step as f32 / SWIPE_STEPS as f32;
        rig.touch(TouchPhase::Moved, x, y_px);
        rig.frame();
    }
    rig.touch(TouchPhase::Ended, SWIPE_TO_X, y_px);
    rig.frames(4);
}

#[test]
fn the_finger_on_the_tone_base_shows_the_frame_with_another_base_once_the_section_stands_in_the_safe_area() {
    let safe_area = SafeAreaInsets::try_from_insets(OWNER_INSETS).expect("зона");
    let Some(mut rig) = calibrating_on_the_owner_phone(safe_area) else {
        return;
    };
    let preset = rig.library.active_look().preset_recipe.settings;
    rig.assert_shows(ToneBaseChoice::Agx, &preset, "до свайпа: основа пресета");
    assert!(
        rig.reference_gap((ToneBaseChoice::Agx, &preset), (ToneBaseChoice::Aces, &preset))
            > 8.0 * viewfinder_rig::TOLERANCE,
        "эталоны моделей различимы"
    );
    swipe_left_at(&mut rig, SAFE_HEADER_Y);
    rig.assert_shows(ToneBaseChoice::Aces, &preset, "после свайпа пальцем: кадр моделью ACES");
    rig.finish();
}

#[test]
fn without_the_safe_area_the_same_gesture_belongs_to_the_system_and_the_picture_stays() {
    let Some(mut rig) = calibrating_on_the_owner_phone(SafeAreaInsets::NONE) else {
        return;
    };
    let preset = rig.library.active_look().preset_recipe.settings;
    swipe_left_at(&mut rig, OLD_HEADER_Y);
    rig.assert_shows(
        ToneBaseChoice::Agx,
        &preset,
        "палец из полосы шторки картинку не меняет",
    );
    assert_eq!(
        rig.board
            .latest()
            .expect("отчёт")
            .diagnostics
            .touches_ignored_in_system_gesture_zone,
        1
    );
    rig.finish();
}
