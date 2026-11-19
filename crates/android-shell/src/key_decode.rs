//! Разбор `KeyEvent` Android в нейтральное [`HardwareKeyEvent`].
//!
//! Kotlin пересылает сырые поля `KeyEvent` (код клавиши, действие, `repeatCount`, время),
//! а смысл решается здесь — без Android, поэтому тестируется на ПК.
//!
//! **Часы.** У `KeyEvent` нет наносекундного геттера (в android.jar SDK 36 он только у
//! `MotionEvent`), поэтому Kotlin передаёт `KeyEvent.getEventTime()` — миллисекунды в шкале
//! `SystemClock.uptimeMillis()`. Это `CLOCK_MONOTONIC` (стоит в глубоком сне), то есть та же
//! шкала, что у касаний (`AMotionEvent_getEventTime`, `System.nanoTime`) и у
//! [`shell_contract::clock::MonotonicNanos`]. Здесь миллисекунды переводятся в наносекунды
//! умножением на 1 000 000 (с насыщением на границе `i64`): шкала та же, точность миллисекундная —
//! для кнопки затвора достаточно. `SystemClock.uptimeNanos()` вместо события не берём: это время
//! вызова, а не момента нажатия, и оно врало бы при задержке главного потока.
//!
//! Значения кодов — `android.view.KeyEvent.KEYCODE_*` / `ACTION_*` (android.jar SDK 36).

use shell_contract::clock::MonotonicNanos;
use shell_contract::event_queue::SendOutcome;
use shell_contract::hardware_key::{HardwareKey, HardwareKeyEvent, KeyPhase, KeyRepeat};

/// Наносекунд в миллисекунде (перевод шкалы `uptimeMillis` в [`MonotonicNanos`]).
const NANOS_PER_MILLI: i64 = 1_000_000;

/// Коды `KeyEvent`.
mod code {
    pub const KEYCODE_VOLUME_UP: i32 = 24;
    pub const KEYCODE_VOLUME_DOWN: i32 = 25;
    pub const ACTION_DOWN: i32 = 0;
    pub const ACTION_UP: i32 = 1;
}

/// Поля `KeyEvent`, как их передал Kotlin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKeyEvent {
    /// `getKeyCode()`.
    pub key_code: i32,
    /// `getAction()`.
    pub action: i32,
    /// `getRepeatCount()`: 0 для первого нажатия.
    pub repeat_count: i32,
    /// `getEventTime()` в миллисекундах (`uptimeMillis`, `CLOCK_MONOTONIC`).
    pub event_time_ms: i64,
}

/// Итог разбора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedKey {
    /// Клавиша, которую оболочка отдаёт приложению (и значит, событие потребляется).
    Handled(HardwareKeyEvent),
    /// Не наша клавиша: её обрабатывает система как обычно.
    NotOurKey { key_code: i32 },
}

/// Событие противоречит контракту Android — ошибка платформы или обвязки, не нажатие.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyDecodeError {
    #[error("неизвестное действие клавиши {action} (клавиша {key_code})")]
    UnknownAction { key_code: i32, action: i32 },
    #[error("отрицательный repeatCount {repeat_count} (клавиша {key_code})")]
    NegativeRepeatCount { key_code: i32, repeat_count: i32 },
}

/// Разбирает событие клавиши.
pub fn decode_key_event(raw: &RawKeyEvent) -> Result<DecodedKey, KeyDecodeError> {
    let key = match raw.key_code {
        code::KEYCODE_VOLUME_UP => HardwareKey::VolumeUp,
        code::KEYCODE_VOLUME_DOWN => HardwareKey::VolumeDown,
        key_code => return Ok(DecodedKey::NotOurKey { key_code }),
    };
    let phase = match raw.action {
        code::ACTION_DOWN => KeyPhase::Pressed,
        code::ACTION_UP => KeyPhase::Released,
        action => {
            return Err(KeyDecodeError::UnknownAction {
                key_code: raw.key_code,
                action,
            });
        }
    };
    let repeat_count = u32::try_from(raw.repeat_count).map_err(|_| KeyDecodeError::NegativeRepeatCount {
        key_code: raw.key_code,
        repeat_count: raw.repeat_count,
    })?;
    // Автоповтор бывает только у нажатия; у отпускания repeatCount бессмыслен — считаем его первым.
    let repeat = match (phase, repeat_count) {
        (KeyPhase::Pressed, count) if count > 0 => KeyRepeat::AutoRepeat { count },
        _ => KeyRepeat::Initial,
    };
    Ok(DecodedKey::Handled(HardwareKeyEvent {
        key,
        phase,
        repeat,
        time: MonotonicNanos::from_nanos(raw.event_time_ms.saturating_mul(NANOS_PER_MILLI)),
    }))
}

/// Что ответить Android после попытки отдать нашу клавишу приложению.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyConsumption {
    /// Событие в очереди приложения: Activity потребляет клавишу (громкость не меняется).
    Consumed,
    /// Приложение событие не получило: клавиша остаётся системе, чтобы нажатие не пропало
    /// молча (система хотя бы изменит громкость). Потерю уже посчитала и залогировала очередь.
    LeftToSystem,
}

impl KeyConsumption {
    /// Решение по итогу отправки в очередь событий оболочки. Потреблять можно только
    /// доставленное событие: при полной очереди или умершем маршрутизаторе «съесть» клавишу
    /// значило бы потерять нажатие затвора без следа для пользователя.
    pub fn after_send(outcome: SendOutcome) -> Self {
        match outcome {
            SendOutcome::Delivered => Self::Consumed,
            // `TouchDropped` для клавиши невозможен (это итог только касаний), но если бы
            // случился — событие не доставлено, значит клавиша системе.
            SendOutcome::TouchDropped | SendOutcome::CriticalLost | SendOutcome::ReceiverGone => Self::LeftToSystem,
        }
    }

    /// Ответ `dispatchKeyEvent`: `true` — событие потреблено.
    pub fn is_consumed(self) -> bool {
        self == Self::Consumed
    }
}

#[cfg(test)]
#[path = "key_decode_tests.rs"]
mod tests;
