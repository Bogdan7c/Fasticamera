//! Секция `viewfinder.continuity`: как долго держать последний кадр, пока камера переключается, и когда
//! показывать новую (сессия 13, решения 110 и 113: «ноль чёрных кадров» при смене объектива).
//!
//! Числа — оценки (`[estimate]`, «подгонка, не Apple»): замер времени переключения на телефоне
//! (закрытие ≈ 150 мс, открытие → первый кадр 270–400 мс, всего ≈ 0,45–0,6 с) говорит, что 1,5 с — запас
//! втрое; уточнять в 13a.

use serde::{Deserialize, Serialize};

/// Наибольшее значение любого срока, мс (минута): больше — почти наверняка ошибка единиц.
pub const MAX_CONTINUITY_MS: u32 = 60_000;

/// Наибольшее окно последних кадров в диагностике (~28 минут при 60 к/с): больше — почти наверняка ошибка.
pub const MAX_RECENT_FRAMES: usize = 100_000;

/// Проверять ли яркость нового потока перед тем, как показать его вместо удержанного кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealExposureCheck {
    /// Не проверять: достаточно, чтобы был известен цвет (AWB).
    Ignore,
    /// Ждать, пока наш AE измерил сцену нового потока и показанный уровень отличается от цели не больше чем
    /// на `max_exposure_error_ev`. Если AE ничего не может измерить (ручная экспозиция, нет парных
    /// результатов, область вне сетки, вендорский AE), ожидание не включается — иначе кадры ждали бы
    /// весь `max_hold_ms` зря. По умолчанию.
    #[default]
    MeasuredAndPlausible,
}

/// Секция `viewfinder.continuity`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ContinuityConfig {
    /// Сколько мс держать последний кадр, пока новый поток не готов. По истечении новый кадр
    /// показывается как есть (удержание не длится вечно). `[estimate]`: 1500.
    pub max_hold_ms: u32,
    /// Сколько мс держать последний кадр, если нового потока нет вовсе (камера недоступна). По истечении
    /// — честный чёрный (недоступность показывает экран). `[estimate]`: 3000.
    pub max_hold_without_stream_ms: u32,
    /// Проверка яркости перед показом нового потока.
    pub reveal_exposure: RevealExposureCheck,
    /// Допустимое отличие показанного уровня от цели AE, EV (при `MeasuredAndPlausible`). `[estimate]`: 1,0.
    pub max_exposure_error_ev: f32,
    /// Сколько последних показанных кадров помнить в диагностике (метки живой/переходный/чёрный и
    /// кадрирование каждого). 0 — не помнить (счётчики остаются). 120 — две секунды при 60 к/с.
    pub recent_frames: usize,
}

impl Default for ContinuityConfig {
    fn default() -> Self {
        Self {
            max_hold_ms: 1_500,
            max_hold_without_stream_ms: 3_000,
            reveal_exposure: RevealExposureCheck::default(),
            max_exposure_error_ev: 1.0,
            recent_frames: 120,
        }
    }
}

/// Что не так с секцией.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ContinuityConfigError {
    /// Срок нулевой или абсурдно большой.
    #[error("viewfinder.continuity.{field} должен лежать в 1..={MAX_CONTINUITY_MS} мс, а не {value}")]
    DurationOutOfRange {
        /// Имя поля.
        field: &'static str,
        /// Значение.
        value: u32,
    },
    /// Без потока ждать меньше, чем с потоком: честный чёрный наступил бы раньше срока удержания.
    #[error(
        "viewfinder.continuity.max_hold_without_stream_ms ({without_stream}) не может быть меньше max_hold_ms ({hold})"
    )]
    NoStreamShorterThanHold {
        /// `max_hold_ms`.
        hold: u32,
        /// `max_hold_without_stream_ms`.
        without_stream: u32,
    },
    /// Окно кадров абсурдно большое.
    #[error("viewfinder.continuity.recent_frames не больше {MAX_RECENT_FRAMES}, а не {0}")]
    RecentFramesTooLarge(usize),
    /// Допуск яркости не конечное положительное число.
    #[error("viewfinder.continuity.max_exposure_error_ev должен быть конечным и положительным, а не {0}")]
    ExposureErrorNotPositive(f32),
}

impl ContinuityConfig {
    /// Проверяет секцию.
    pub fn validate(&self) -> Result<(), ContinuityConfigError> {
        for (field, value) in [
            ("max_hold_ms", self.max_hold_ms),
            ("max_hold_without_stream_ms", self.max_hold_without_stream_ms),
        ] {
            if value == 0 || value > MAX_CONTINUITY_MS {
                return Err(ContinuityConfigError::DurationOutOfRange { field, value });
            }
        }
        if self.max_hold_without_stream_ms < self.max_hold_ms {
            return Err(ContinuityConfigError::NoStreamShorterThanHold {
                hold: self.max_hold_ms,
                without_stream: self.max_hold_without_stream_ms,
            });
        }
        if !(self.max_exposure_error_ev.is_finite() && self.max_exposure_error_ev > 0.0) {
            return Err(ContinuityConfigError::ExposureErrorNotPositive(
                self.max_exposure_error_ev,
            ));
        }
        if self.recent_frames > MAX_RECENT_FRAMES {
            return Err(ContinuityConfigError::RecentFramesTooLarge(self.recent_frames));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_the_documented_estimates() {
        let config = ContinuityConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!((config.max_hold_ms, config.max_hold_without_stream_ms), (1_500, 3_000));
        assert_eq!(config.reveal_exposure, RevealExposureCheck::MeasuredAndPlausible);
    }

    #[test]
    fn each_broken_value_is_rejected_with_its_own_error() {
        let base = ContinuityConfig::default();
        let zero_hold = ContinuityConfig { max_hold_ms: 0, ..base };
        assert_eq!(
            zero_hold.validate(),
            Err(ContinuityConfigError::DurationOutOfRange {
                field: "max_hold_ms",
                value: 0
            })
        );
        let huge = ContinuityConfig {
            max_hold_without_stream_ms: MAX_CONTINUITY_MS + 1,
            ..base
        };
        assert!(matches!(
            huge.validate(),
            Err(ContinuityConfigError::DurationOutOfRange {
                field: "max_hold_without_stream_ms",
                ..
            })
        ));
        let inverted = ContinuityConfig {
            max_hold_ms: 2_000,
            max_hold_without_stream_ms: 1_000,
            ..base
        };
        assert_eq!(
            inverted.validate(),
            Err(ContinuityConfigError::NoStreamShorterThanHold {
                hold: 2_000,
                without_stream: 1_000
            })
        );
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let broken = ContinuityConfig {
                max_exposure_error_ev: bad,
                ..base
            };
            assert!(
                matches!(
                    broken.validate(),
                    Err(ContinuityConfigError::ExposureErrorNotPositive(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn json_fills_missing_fields_and_rejects_unknown_ones() {
        let parsed: ContinuityConfig =
            serde_json::from_str(r#"{"max_hold_ms":800,"reveal_exposure":"ignore"}"#).expect("секция читается");
        assert_eq!(parsed.max_hold_ms, 800);
        assert_eq!(parsed.reveal_exposure, RevealExposureCheck::Ignore);
        assert_eq!(parsed.max_hold_without_stream_ms, 3_000, "остальное — умолчание");
        assert!(serde_json::from_str::<ContinuityConfig>(r#"{"max_hold":800}"#).is_err());
    }
}
