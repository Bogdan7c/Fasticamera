//! Общие помощники тестов панели белого через настоящие касания и кадры экрана: факты, долгое нажатие, цели
//! касаний из публичного списка, выборка эффектов камеры.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin};
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;

use super::test_rig::*;
use super::white_targets::{WhiteCardTargets, WhiteTouchTargets};
use crate::action::ScreenAction;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::CameraFacts;
use crate::test_support::live_facts;
use crate::white_facts::WhiteFacts;

/// Действующее белое с происхождением.
pub(super) fn held(kelvin: f64, duv: f64, origin: WhiteOrigin) -> HeldWhite {
    HeldWhite {
        setting: WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(kelvin).expect("температура теста"),
            tint: Tint::from_duv(duv).expect("оттенок теста"),
        },
        origin,
    }
}

/// Факты камеры с белым `white`.
pub(super) fn facts_with(white: WhiteFacts) -> CameraFacts {
    CameraFacts { white, ..live_facts() }
}

/// Факты с белым `kelvin`/`duv` и происхождением `origin`, остальное — запуск.
pub(super) fn facts_held(kelvin: f64, duv: f64, origin: WhiteOrigin) -> CameraFacts {
    facts_with(WhiteFacts {
        held: held(kelvin, duv, origin),
        ..WhiteFacts::STARTING
    })
}

/// Применить факты и дать экрану пару кадров.
pub(super) fn set_facts(rig: &mut Rig, facts: CameraFacts) {
    rig.screen.handle_action(ScreenAction::CameraFactsChanged(facts));
    rig.run(2 * FRAME_MS);
}

/// Цели касаний белого (раскладка уже есть).
pub(super) fn targets(rig: &Rig) -> WhiteTouchTargets {
    rig.screen.white_touch_targets().expect("раскладка есть")
}

/// Цели открытой карточки белого.
pub(super) fn card_targets(rig: &Rig) -> WhiteCardTargets {
    targets(rig).card.expect("карточка белого открыта")
}

/// Центр кнопки белого у затвора (с подъёмом, если открыта карточка и анимация кончилась).
pub(super) fn white_button(rig: &Rig) -> Point {
    targets(rig).button.center()
}

/// Открыть карточку белого настоящим тапом по кнопке и дождаться конца морфа.
pub(super) fn open_card(rig: &mut Rig) {
    let button = white_button(rig);
    rig.tap(button);
    rig.run(700);
    rig.effects();
}

/// Палец лёг в `point`, лежит `hold_ms` (кадры идут, таймер долгого нажатия тикает), поднят, ещё кадр.
pub(super) fn long_press(rig: &mut Rig, point: Point, hold_ms: i64) {
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, point, start);
    let end = start + hold_ms;
    while rig.now_ms < end {
        rig.now_ms += FRAME_MS;
        rig.frame();
    }
    rig.touch(TouchPhase::Ended, point, rig.now_ms);
    rig.now_ms += 10;
    rig.frame();
}

/// Эффекты, которые идут камере (`Effect::Camera`).
pub(super) fn camera_intents(effects: &[Effect]) -> Vec<ScreenCameraIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Camera(intent) => Some(*intent),
            _ => None,
        })
        .collect()
}
