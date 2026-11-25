//! Числа элементов «внимание»: мерцание ожидания и тряска отказа (модуль [`crate::attention`]).
//!
//! Все величины — `[estimate]` (подгонка, не Apple): в эталоне iOS 26 аналогов нет, числа взяты из слов
//! владельца (плана white-balance, WB32) и правятся по телефону.

use serde::{Deserialize, Serialize};

use super::elements::positive;
use super::error::TokenError;

/// Параметры мерцания ожидания и тряски отказа.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AttentionTokens {
    /// Период мерцания ожидания, с: полный цикл «1 → нижний уровень → 1». `[estimate]` 1,0.
    pub waiting_period_s: f64,
    /// Нижний уровень прозрачности мерцания, 0..1. `[estimate]` 0,35: число видно, но не пропадает.
    pub waiting_min_opacity: f64,
    /// За сколько секунд мерцание мягко возвращается к полной прозрачности по окончании ожидания (и сколько
    /// длится вход в мерцание, если оно началось поверх возврата). `[estimate]` 0,25.
    pub waiting_settle_s: f64,
    /// Амплитуда тряски отказа, pt (в начале; затухает). `[estimate]` 6.
    pub shake_amplitude_pt: f64,
    /// Сколько полных колебаний в тряске. `[estimate]` 3.
    pub shake_periods: f64,
    /// Длительность тряски, с. `[estimate]` 0,4.
    pub shake_duration_s: f64,
    /// Reduce Motion: вместо тряски — затемнение до этого уровня прозрачности (0..1) и возврат. `[estimate]` 0,5.
    pub reduced_dim_opacity: f64,
    /// Reduce Motion: длительность затемнения-возврата, с. `[estimate]` 0,25.
    pub reduced_dim_duration_s: f64,
}

impl Default for AttentionTokens {
    fn default() -> Self {
        Self {
            waiting_period_s: 1.0,
            waiting_min_opacity: 0.35,
            waiting_settle_s: 0.25,
            shake_amplitude_pt: 6.0,
            shake_periods: 3.0,
            shake_duration_s: 0.4,
            reduced_dim_opacity: 0.5,
            reduced_dim_duration_s: 0.25,
        }
    }
}

impl AttentionTokens {
    /// Длительности и амплитуда положительны, уровни прозрачности лежат строго между 0 и 1.
    pub fn validate(&self) -> Result<(), TokenError> {
        positive("attention.waiting_period_s", self.waiting_period_s)?;
        positive("attention.waiting_settle_s", self.waiting_settle_s)?;
        positive("attention.shake_amplitude_pt", self.shake_amplitude_pt)?;
        positive("attention.shake_periods", self.shake_periods)?;
        positive("attention.shake_duration_s", self.shake_duration_s)?;
        positive("attention.reduced_dim_duration_s", self.reduced_dim_duration_s)?;
        for (name, value) in [
            ("attention.waiting_min_opacity", self.waiting_min_opacity),
            ("attention.reduced_dim_opacity", self.reduced_dim_opacity),
        ] {
            // Строго меньше 1: иначе «мерцание» или «затемнение» ничего не меняют; не меньше 0.
            if !value.is_finite() || !(0.0..1.0).contains(&value) {
                return Err(TokenError::OutOfRange { name, value });
            }
        }
        Ok(())
    }
}
