//! Аппаратные клавиши телефона в нейтральном виде.
//!
//! Сейчас экрану камеры нужны только клавиши громкости (затвор, решение v1 из `scope-v1.md`).
//! Оболочка пересылает **нажатие и отпускание как есть**, без жестов: «удержание» (QuickTake,
//! серия) — отдельный жест, который в будущем соберёт потребитель из пары
//! `Pressed`/`Released`. Время — `CLOCK_MONOTONIC`, та же шкала, что у касаний
//! ([`crate::touch::TouchSample::time`]): `KeyEvent.getEventTime()` (uptimeMillis) и
//! `MotionEvent` берут время из одних и тех же часов; миллисекунды переводит оболочка.

use serde::Serialize;

use crate::clock::MonotonicNanos;

/// Какая аппаратная клавиша. Список расширяется по мере появления потребителей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareKey {
    /// Клавиша «громкость вверх» (`KEYCODE_VOLUME_UP`).
    VolumeUp,
    /// Клавиша «громкость вниз» (`KEYCODE_VOLUME_DOWN`).
    VolumeDown,
}

/// Фаза клавиши.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPhase {
    /// Клавишу нажали (включая автоповтор при удержании — см. [`KeyRepeat`]).
    Pressed,
    /// Клавишу отпустили.
    Released,
}

/// Первое это нажатие или автоповтор от удержания.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyRepeat {
    /// Первое нажатие (`KeyEvent.getRepeatCount() == 0`) и любое отпускание.
    Initial,
    /// Автоповтор системы при удержании; `count` — `getRepeatCount()` (1, 2, …).
    AutoRepeat { count: u32 },
}

/// Одно событие аппаратной клавиши.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HardwareKeyEvent {
    /// Какая клавиша.
    pub key: HardwareKey,
    /// Нажата или отпущена. Отменённое системой отпускание (`FLAG_CANCELED`) пока не
    /// различается: потребителя отпускания в v1 нет (известное ограничение, сессия 12).
    pub phase: KeyPhase,
    /// Первое нажатие или автоповтор удержания.
    pub repeat: KeyRepeat,
    /// Момент события по часам касаний (`CLOCK_MONOTONIC`), точность — миллисекунда.
    pub time: MonotonicNanos,
}

impl HardwareKeyEvent {
    /// Это первое физическое нажатие (не автоповтор и не отпускание) — момент для затвора.
    pub fn is_initial_press(&self) -> bool {
        self.phase == KeyPhase::Pressed && self.repeat == KeyRepeat::Initial
    }
}

#[cfg(test)]
#[path = "hardware_key_tests.rs"]
mod tests;
