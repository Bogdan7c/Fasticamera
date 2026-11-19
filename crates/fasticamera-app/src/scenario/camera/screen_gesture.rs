//! Жест, который сценарий `screen` просит выполнить у скрипта-драйвера (`scripts/smoke_driver.py`).
//!
//! Сценарий ничего не нажимает сам: он пишет стадию `screen_gesture` с полем `gesture` (этот тип в JSON) и ждёт
//! результата на экране камеры. Драйвер читает `gesture` и вызывает `adb` (тап, долгое нажатие, свайп, клавиша
//! громкости). Координаты — доли экрана, как в драйвере; числа берутся из плана касаний конфига
//! ([`crate::screen_scenario_config::ScreenTouchPlan`]), других источников у них нет.
//!
//! Контракт с Python (`smoke_driver.py`, `StepKind.STAGE_GESTURE`): поле `kind` — `tap` | `long_press` | `swipe` |
//! `volume_up`; точки — `x`, `y` (и `end_x`, `end_y` у свайпа); длительность — `hold_ms` (долгое нажатие) или
//! `duration_ms` (свайп). Менять имена полей можно только вместе с драйвером и его тестами.

use serde::Serialize;

use crate::screen_scenario_config::{HoldGesture, ScreenPoint, SwipeGesture};

/// Жест на устройстве.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Gesture {
    /// Короткое касание.
    Tap { x: f64, y: f64 },
    /// Удержание пальца на месте.
    LongPress { x: f64, y: f64, hold_ms: u64 },
    /// Проведение пальцем.
    Swipe {
        x: f64,
        y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: u64,
    },
    /// Клавиша громкости «+» (`KEYCODE_VOLUME_UP`): затвор с клавиши.
    VolumeUp,
}

impl Gesture {
    pub fn tap(at: ScreenPoint) -> Self {
        Self::Tap { x: at.x, y: at.y }
    }

    pub fn long_press(hold: HoldGesture) -> Self {
        Self::LongPress {
            x: hold.at.x,
            y: hold.at.y,
            hold_ms: hold.hold_ms,
        }
    }

    pub fn swipe(swipe: SwipeGesture) -> Self {
        Self::Swipe {
            x: swipe.from.x,
            y: swipe.from.y,
            end_x: swipe.to.x,
            end_y: swipe.to.y,
            duration_ms: swipe.duration_ms,
        }
    }

    /// Жест как значение поля стадии.
    pub fn to_stage_value(self) -> serde_json::Value {
        // Сериализация простого enum не падает; на случай сбоя — запись-объяснение, а не паника и не тишина.
        serde_json::to_value(self)
            .unwrap_or_else(|error| serde_json::json!({ "serialization_error": error.to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::screen::GESTURE_STAGE;
    use super::*;

    /// Контракт с драйвером в том виде, в каком его читает Python (`scripts/tests/test_smoke_screen_scenario.py`).
    const CONTRACT: &str = include_str!("../../../../../scripts/tests/fixtures/smoke/screen_gesture_contract.json");
    /// Исходник драйвера: таблица шагов сценария `screen` называет стадию, которую выдаёт Rust.
    const DRIVER_SOURCE: &str = include_str!("../../../../../scripts/smoke_driver.py");

    fn contract() -> serde_json::Value {
        serde_json::from_str(CONTRACT).expect("фикстура контракта — верный JSON")
    }

    #[test]
    fn each_gesture_serialises_to_the_contract_the_driver_reads() {
        let contract = contract();
        assert_eq!(
            Gesture::tap(ScreenPoint::new(0.5, 0.25)).to_stage_value(),
            contract["tap"]
        );
        let hold = HoldGesture {
            at: ScreenPoint::new(0.1, 0.2),
            hold_ms: 800,
        };
        assert_eq!(Gesture::long_press(hold).to_stage_value(), contract["long_press"]);
        let swipe = SwipeGesture {
            from: ScreenPoint::new(0.5, 0.6),
            to: ScreenPoint::new(0.5, 0.4),
            duration_ms: 300,
        };
        assert_eq!(Gesture::swipe(swipe).to_stage_value(), contract["swipe"]);
        assert_eq!(Gesture::VolumeUp.to_stage_value(), contract["volume_up"]);
    }

    #[test]
    fn the_driver_table_waits_for_the_stage_this_scenario_emits() {
        // Нет общего механизма имён стадий Rust <-> Python: сверяем текстом таблицы драйвера.
        let expected = format!("\"screen\": {{\"{GESTURE_STAGE}\": (Step(StepKind.STAGE_GESTURE),)}}");
        assert!(
            DRIVER_SOURCE.contains(&expected),
            "в SCENARIO_STEPS драйвера нет записи {expected}"
        );
    }
}
