//! План шагов сценария `screen`: какой жест просить у скрипта и что должно измениться после него.
//!
//! Порядок важен — шаги строят друг на друге состояние экрана (после долгого нажатия стоит блокировка, после
//! свайпа вверх открыто меню). Конец плана нейтральный: блокировка снята, панели закрыты, EV сброшен.
//!
//! # Как доказывается состояние экрана
//! Отчёт экрана несёт снимок наблюдаемого состояния ([`crate::camera_screen_source::ScreenStateSnapshot`]), поэтому
//! «меню открыто», «панель Exposure открыта», «плашка AE/AF LOCK видна», шаги EV и режим читаются НАПРЯМУЮ
//! ([`StateCheck::Panel`] и соседи). Тап-пробы по кадру остались вторым, независимым доказательством:
//! * тап по видоискателю при открытой панели только закрывает её и камере ничего не шлёт (`reduce/viewfinder.rs`,
//!   решение 94), а при закрытой наводит фокус и шлёт намерение, которое должно дойти до HAL (связь с камерой);
//! * ползунок EV шлёт камере `SetExposureBias`, движок держит ненулевой EV.
//!
//! Косвенных шагов не осталось: у каждого есть прямая проверка состояния экрана ([`PlannedStep::inferred`]
//! заполняется только если появится шаг без неё).

use camera_contract::three_a::AeAfLock;

use super::screen_expect::{Count, Expectation, Metric, StateCheck};
use super::screen_gesture::Gesture;
use super::screen_sensor_point::FramePoint;
use crate::camera_screen_source::{ModeKind, PanelKind, Presence, ShutterPlacement};
use crate::screen_scenario_config::{ScreenPoint, ScreenTouchPlan};

/// Один шаг: жест + ожидание.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedStep {
    /// Имя для отчёта и для поля `step` стадии (латиница, без пробелов).
    pub name: &'static str,
    /// Что проверяется, по-русски (в отчёте провала).
    pub purpose: &'static str,
    /// Какой жест просить у скрипта.
    pub gesture: Gesture,
    pub expect: Expectation,
    /// Если состояние экрана доказывается косвенно — как именно (иначе `None`: есть прямая проверка снимка).
    pub inferred: Option<&'static str>,
}

/// Тап, обязанный дойти до HAL: экран распознал тап, камере ушло одно намерение, движок отправил один триггер.
fn tap_reaching_the_hal() -> Expectation {
    Expectation::new()
        .count(Metric::Taps, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::Exactly(1))
        .count(Metric::FocusTriggers, Count::Exactly(1))
}

/// Тап, обязанный только закрыть панель: распознан, камере ничего.
fn tap_that_only_closes_a_panel() -> Expectation {
    Expectation::new()
        .count(Metric::Taps, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::Exactly(0))
        .count(Metric::FocusTriggers, Count::Exactly(0))
}

/// Свайп, после которого камере ничего не уходит (панели и режимы — дело экрана).
fn swipe_without_camera_effect() -> Expectation {
    Expectation::new()
        .count(Metric::Pans, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::Exactly(0))
}

/// Нажатие круглой кнопки без команды камере.
fn button_without_camera_effect() -> Expectation {
    Expectation::new()
        .count(Metric::ButtonPresses, Count::Exactly(1))
        .count(Metric::CameraIntents, Count::Exactly(0))
}

/// Все шаги сценария по порядку.
pub fn planned_steps(plan: &ScreenTouchPlan) -> Vec<PlannedStep> {
    let mut steps = focus_and_lock_steps(plan);
    steps.extend(menu_steps(plan));
    steps.extend(exposure_steps(plan));
    steps.extend(mode_and_shutter_steps(plan));
    steps
}

fn focus_and_lock_steps(plan: &ScreenTouchPlan) -> Vec<PlannedStep> {
    // Тап фокуса доказывается дважды: счётчиками (дошёл до HAL) и МЕСТОМ (точка интереса движка там, куда нажали).
    let focus_tap = |name, purpose, at, in_frame: ScreenPoint| PlannedStep {
        name,
        purpose,
        gesture: Gesture::tap(at),
        expect: tap_reaching_the_hal().state(StateCheck::SensorPoint {
            frame: FramePoint {
                u: in_frame.x as f32,
                v: in_frame.y as f32,
            },
            tolerance: plan.focus_point_tolerance as f32,
        }),
        inferred: None,
    };
    vec![
        focus_tap(
            "focus_tap_center",
            "тап фокуса в центр кадра дошёл до HAL и указал в нужное место сенсора",
            plan.focus_center,
            plan.focus_targets.center,
        ),
        focus_tap(
            "focus_tap_top_left",
            "тап фокуса в левый верх кадра дошёл до HAL и указал в нужное место сенсора",
            plan.focus_top_left,
            plan.focus_targets.top_left,
        ),
        focus_tap(
            "focus_tap_bottom_right",
            "тап фокуса в правый низ кадра дошёл до HAL и указал в нужное место сенсора",
            plan.focus_bottom_right,
            plan.focus_targets.bottom_right,
        ),
        PlannedStep {
            name: "lock_long_press",
            purpose: "долгое нажатие блокирует AE/AF: команда дошла до HAL, движок держит блокировку, плашка AE/AF LOCK видна",
            gesture: Gesture::long_press(plan.lock_press),
            expect: Expectation::new()
                .count(Metric::LongPresses, Count::Exactly(1))
                .count(Metric::CameraIntents, Count::Exactly(1))
                .count(Metric::FocusTriggers, Count::Exactly(1))
                .state(StateCheck::Lock(AeAfLock::Locked))
                .state(StateCheck::LockPlaque(Presence::Shown)),
            inferred: None,
        },
        PlannedStep {
            name: "unlock_tap",
            purpose: "тап после блокировки снимает её: новая точка дошла до HAL, движок разблокирован, плашка скрыта",
            gesture: Gesture::tap(plan.focus_center),
            expect: tap_reaching_the_hal()
                .state(StateCheck::Lock(AeAfLock::Unlocked))
                .state(StateCheck::LockPlaque(Presence::Hidden)),
            inferred: None,
        },
    ]
}

fn menu_steps(plan: &ScreenTouchPlan) -> Vec<PlannedStep> {
    vec![
        PlannedStep {
            name: "menu_open_swipe",
            purpose: "свайп вверх по видоискателю открыл меню Controls: панель ControlsMenu, затвор под шторкой",
            gesture: Gesture::swipe(plan.menu_open_swipe),
            expect: swipe_without_camera_effect()
                .state(StateCheck::Panel(PanelKind::ControlsMenu))
                .state(StateCheck::Shutter(ShutterPlacement::HiddenUnderDrawer)),
            inferred: None,
        },
        PlannedStep {
            name: "menu_probe_tap_opened",
            purpose: "тап по кадру при открытом меню только закрывает его: панель Closed, камере не шлёт намерение фокуса",
            gesture: Gesture::tap(plan.probe_tap),
            expect: tap_that_only_closes_a_panel()
                .state(StateCheck::Panel(PanelKind::Closed))
                .state(StateCheck::Shutter(ShutterPlacement::Visible)),
            inferred: None,
        },
        PlannedStep {
            name: "menu_reopen_swipe",
            purpose: "свайп вверх снова открывает меню: панель ControlsMenu, затвор под шторкой",
            gesture: Gesture::swipe(plan.menu_open_swipe),
            expect: swipe_without_camera_effect()
                .state(StateCheck::Panel(PanelKind::ControlsMenu))
                .state(StateCheck::Shutter(ShutterPlacement::HiddenUnderDrawer)),
            inferred: None,
        },
        PlannedStep {
            name: "menu_close_swipe",
            purpose: "свайп вниз по шторке закрыл меню: панель Closed, затвор виден",
            gesture: Gesture::swipe(plan.menu_close_swipe),
            expect: swipe_without_camera_effect()
                .state(StateCheck::Panel(PanelKind::Closed))
                .state(StateCheck::Shutter(ShutterPlacement::Visible)),
            inferred: None,
        },
        PlannedStep {
            name: "menu_probe_tap_closed",
            purpose: "после закрытия меню свайпом тап по кадру наводит фокус и доходит до HAL",
            gesture: Gesture::tap(plan.probe_tap),
            expect: tap_reaching_the_hal(),
            inferred: None,
        },
    ]
}

fn exposure_steps(plan: &ScreenTouchPlan) -> Vec<PlannedStep> {
    vec![
        PlannedStep {
            name: "ev_icon_tap",
            purpose: "тап по значку EV открыл карточку Exposure: панель ExposurePanel, ряд затвора поднят над карточкой",
            gesture: Gesture::tap(plan.ev_icon),
            expect: button_without_camera_effect()
                .state(StateCheck::Panel(PanelKind::ExposurePanel))
                .state(StateCheck::Shutter(ShutterPlacement::RaisedAboveCard)),
            inferred: None,
        },
        PlannedStep {
            name: "ev_slider_swipe",
            // «Дошёл до камеры» = желаемое EV лежит в состоянии ДВИЖКА (`three_a.controls.exposure_bias`). Применил ли его
            // HAL, статус не сообщает (надёжного признака применения нет): шаг доказывает связь «экран → движок».
            purpose: "свайп по шкале EV сдвинул шкалу экрана (exposure_steps != 0) и дошёл до движка камеры: он держит ненулевое желаемое EV (применение HAL не проверяется)",
            gesture: Gesture::swipe(plan.ev_slider_swipe),
            expect: Expectation::new()
                .count(Metric::Pans, Count::Exactly(1))
                .count(Metric::CameraIntents, Count::AtLeast(1))
                .state(StateCheck::ExposureBiasIsSet)
                .state(StateCheck::ScreenExposureStepsAreSet),
            inferred: None,
        },
        PlannedStep {
            name: "ev_reset_tap",
            purpose: "кнопка сброса вернула EV в 0 на шкале экрана и в желаемом состоянии движка (применение HAL не проверяется)",
            gesture: Gesture::tap(plan.ev_reset),
            expect: Expectation::new()
                .count(Metric::ButtonPresses, Count::Exactly(1))
                .count(Metric::CameraIntents, Count::AtLeast(1))
                .state(StateCheck::ExposureBiasIsZero)
                .state(StateCheck::ScreenExposureStepsAreZero),
            inferred: None,
        },
        PlannedStep {
            name: "ev_close_tap",
            purpose: "кнопка «×» закрыла карточку Exposure: панель Closed, затвор вернулся на место",
            gesture: Gesture::tap(plan.ev_close),
            expect: button_without_camera_effect()
                .state(StateCheck::Panel(PanelKind::Closed))
                .state(StateCheck::Shutter(ShutterPlacement::Visible)),
            inferred: None,
        },
        PlannedStep {
            name: "ev_probe_tap_closed",
            purpose: "после закрытия панели Exposure тап по кадру наводит фокус и доходит до HAL",
            gesture: Gesture::tap(plan.probe_tap),
            expect: tap_reaching_the_hal(),
            inferred: None,
        },
    ]
}

fn mode_and_shutter_steps(plan: &ScreenTouchPlan) -> Vec<PlannedStep> {
    vec![
        PlannedStep {
            name: "modes_swipe_left",
            purpose: "горизонтальный свайп справа налево распознан, режим остался Photo (он один), камере ничего",
            gesture: Gesture::swipe(plan.modes_swipe_left),
            expect: swipe_without_camera_effect().state(StateCheck::Mode(ModeKind::Photo)),
            inferred: None,
        },
        PlannedStep {
            name: "modes_swipe_right",
            purpose: "горизонтальный свайп слева направо распознан, режим остался Photo (он один), камере ничего",
            gesture: Gesture::swipe(plan.modes_swipe_right),
            expect: swipe_without_camera_effect().state(StateCheck::Mode(ModeKind::Photo)),
            inferred: None,
        },
        PlannedStep {
            name: "volume_up_shutter",
            purpose: "клавиша громкости сделала фейк-снимок: запрос в журнале, миниатюра показывает снимок",
            gesture: Gesture::VolumeUp,
            expect: Expectation::new()
                .count(Metric::CaptureRequests, Count::Exactly(1))
                .state(StateCheck::ThumbnailShowsLast),
            inferred: None,
        },
    ]
}

#[cfg(test)]
#[path = "screen_steps_tests.rs"]
mod tests;
