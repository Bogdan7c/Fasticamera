//! Ограничитель частоты вибраций: не чаще одного паттерна за `min_interval_ms`.
//!
//! Зачем: быстрый бросок колеса зума проходит десяток рисок за кадр-два, и мотор «смазывается» в
//! гул (решение 112: «не чаще одного тика за 30 мс»). Ограничитель один на весь жест (колесо и
//! щипок делят его), потому что мотор один.
//!
//! # Время
//! Только из меток событий (`MonotonicNanos` касаний и кадров), `Instant::now()` нет: на 60 и
//! 120 Гц решение зависит от меток, а не от того, как часто нас вызвали.
//!
//! # Семантика
//! * первая вибрация проходит всегда;
//! * следующая проходит, если от прошлой **сыгранной** прошло не меньше интервала; подавленная
//!   вибрация счётчик интервала не сдвигает (иначе непрерывный поток событий не пропустил бы ничего);
//! * метка раньше прошлой сыгранной (события пришли не по порядку) считается внутри интервала.

use serde::{Deserialize, Serialize};
use ui_core::time::MonotonicNanos;

use crate::tokens::TokenError;

/// Наносекунд в миллисекунде.
const NANOS_PER_MS: f64 = 1_000_000.0;

/// Число ограничителя: минимальный промежуток между вибрациями.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HapticThrottleTokens {
    /// Наименьший промежуток между вибрациями, мс. `[estimate]` 30 — подгонка, не Apple (решение 112).
    pub min_interval_ms: f64,
}

impl Default for HapticThrottleTokens {
    fn default() -> Self {
        Self { min_interval_ms: 30.0 }
    }
}

impl HapticThrottleTokens {
    /// Интервал конечен и > 0.
    pub fn validate(&self) -> Result<(), TokenError> {
        if self.min_interval_ms.is_finite() && self.min_interval_ms > 0.0 {
            Ok(())
        } else {
            Err(TokenError::NonPositiveSize {
                name: "haptic_throttle.min_interval_ms",
                value: self.min_interval_ms,
            })
        }
    }
}

/// Решение ограничителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleVerdict {
    /// Вибрацию играть.
    Play,
    /// Слишком скоро после прошлой: вибрацию не играть (посчитана).
    Suppress,
}

/// Ограничитель частоты вибраций. Значение (без часов и потоков): живёт в состоянии владельца жеста.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HapticThrottle {
    min_interval_nanos: i64,
    last_played: Option<MonotonicNanos>,
    suppressed: u64,
}

impl HapticThrottle {
    /// Ограничитель с проверенными токенами.
    pub fn new(tokens: &HapticThrottleTokens) -> Result<Self, TokenError> {
        tokens.validate()?;
        Ok(Self {
            min_interval_nanos: (tokens.min_interval_ms * NANOS_PER_MS).round() as i64,
            last_played: None,
            suppressed: 0,
        })
    }

    /// Можно ли сыграть вибрацию в момент `time`. Решение «играть» запоминается как последняя вибрация.
    pub fn admit(&mut self, time: MonotonicNanos) -> ThrottleVerdict {
        let due = match self.last_played {
            None => true,
            Some(last) => time.as_nanos().saturating_sub(last.as_nanos()) >= self.min_interval_nanos,
        };
        if due {
            self.last_played = Some(time);
            ThrottleVerdict::Play
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
            ThrottleVerdict::Suppress
        }
    }

    /// Сколько вибраций подавлено (диагностика и тесты).
    pub fn suppressed_count(&self) -> u64 {
        self.suppressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: i64) -> MonotonicNanos {
        MonotonicNanos::from_nanos(value * 1_000_000)
    }

    fn throttle() -> HapticThrottle {
        HapticThrottle::new(&HapticThrottleTokens::default()).expect("токены по умолчанию верны")
    }

    #[test]
    fn first_haptic_always_plays_and_next_waits_for_interval() {
        let mut throttle = throttle();
        assert_eq!(throttle.admit(ms(1000)), ThrottleVerdict::Play);
        assert_eq!(throttle.admit(ms(1029)), ThrottleVerdict::Suppress);
        assert_eq!(throttle.admit(ms(1030)), ThrottleVerdict::Play);
    }

    #[test]
    fn fast_fling_never_plays_closer_than_the_interval() {
        // Поток событий каждые 4 мс (быстрый бросок колеса): сыгранные не ближе 30 мс друг к другу.
        let mut throttle = throttle();
        let played: Vec<i64> = (0..100)
            .map(|step| step * 4)
            .filter(|moment| throttle.admit(ms(*moment)) == ThrottleVerdict::Play)
            .collect();
        assert!(played.len() >= 10, "бросок должен хоть иногда вибрировать: {played:?}");
        assert!(played.windows(2).all(|pair| pair[1] - pair[0] >= 30), "{played:?}");
        assert_eq!(throttle.suppressed_count(), 100 - played.len() as u64);
    }

    #[test]
    fn suppressed_haptic_does_not_postpone_the_next_one() {
        let mut throttle = throttle();
        assert_eq!(throttle.admit(ms(0)), ThrottleVerdict::Play);
        assert_eq!(throttle.admit(ms(20)), ThrottleVerdict::Suppress);
        // Отсчёт идёт от сыгранной (0), а не от подавленной (20).
        assert_eq!(throttle.admit(ms(31)), ThrottleVerdict::Play);
    }

    #[test]
    fn out_of_order_stamp_counts_as_inside_the_interval() {
        let mut throttle = throttle();
        assert_eq!(throttle.admit(ms(500)), ThrottleVerdict::Play);
        assert_eq!(throttle.admit(ms(480)), ThrottleVerdict::Suppress);
    }

    #[test]
    fn invalid_interval_is_rejected() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let tokens = HapticThrottleTokens { min_interval_ms: bad };
            assert!(HapticThrottle::new(&tokens).is_err(), "{bad}");
        }
    }
}
