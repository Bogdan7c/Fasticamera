//! Элементы «внимание»: мерцание ожидания и тряска отказа (решение 84, правило второго использования).
//!
//! Два маленьких общих эффекта, которые нужны любому экрану, где что-то ждут или отказывают:
//! * [`WaitingFlicker`] — прозрачность плавно ходит между 1 и нижним уровнем, пока мы ждём ответа; по
//!   окончании ожидания мягко возвращается к 1;
//! * [`RefusalShake`] — короткая затухающая тряска вбок: «нет». Повторный запуск начинает её заново.
//!
//! Это чистая логика от времени: часов внутри нет, `now` приходит снаружи ([`ui_core::time::MonotonicNanos`]),
//! поэтому один и тот же поток вызовов даёт один и тот же результат. Экран спрашивает [`WaitingFlicker::is_animating`]
//! / [`RefusalShake::is_animating`], чтобы продолжать рисовать кадры, пока эффект движется, и перестать, когда он
//! завершён. Что именно мерцает или трясётся (число, кнопка), элемент не знает — он отдаёт только числа
//! `opacity` и `offset_x_pt`.
//!
//! Reduce Motion ([`ui_core::time::MotionPreference::Reduced`]): тряска (сдвиг) заменяется коротким
//! затемнением-возвратом без сдвига; мерцание остаётся — оно меняет только прозрачность.

mod flicker;
mod shake;

pub use flicker::WaitingFlicker;
pub use shake::RefusalShake;

use ui_core::time::{MonotonicNanos, UiDuration};

/// Сколько секунд прошло от `start` до `now`; время, пошедшее назад, считается нулём (эффект стоит в начале).
fn elapsed_seconds(start: MonotonicNanos, now: MonotonicNanos) -> f64 {
    let elapsed = UiDuration::between(start, now);
    if elapsed.is_negative() {
        0.0
    } else {
        elapsed.as_secs_f64()
    }
}

/// Плавная «ступенька» 0 → 1 (кубическая): без рывка в начале и конце. Вход зажимается в 0..=1.
fn smoothstep(progress: f64) -> f64 {
    let clamped = progress.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}
