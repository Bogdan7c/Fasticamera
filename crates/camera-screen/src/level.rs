//! Уровень: наклон телефона по вектору гравитации.
//!
//! # Оси Android (`shell_contract::gravity`)
//! Оси — как у `SensorEvent` в естественной (портретной) ориентации: `x` вправо, `y` вверх,
//! `z` из экрана к зрителю, единицы м/с². Вектор показывает «куда вверх в системе телефона»:
//! телефон стоит вертикально — `(0, +g, 0)`, лежит экраном вверх — `(0, 0, +g)`.
//!
//! # Крен
//! Если повернуть телефон в плоскости экрана по часовой на угол θ, «вверх мира» в осях телефона
//! будет `(−g·sin θ, g·cos θ)`. Отсюда `крен = atan2(−x, y)`, положительный по часовой
//! (`LandscapeRight`, верх вправо → +90°). Линия горизонта на экране рисуется повёрнутой на
//! `−крен`, чтобы оставаться горизонтальной в мире.
//!
//! # Отклонение
//! Уровень показывает наклон относительно ближайшей оси (0°, ±90°, 180°): снимающий в альбомной
//! ориентации тоже хочет ровный горизонт. Отклонение лежит в `(−45°, 45°]`.
//!
//! # Плашмя
//! Если гравитация почти вся в оси `z` (телефон лежит или снимает «вниз/вверх»), плоскостного
//! крена нет, и уровень не показывается (`LevelReading::Flat`).

use serde::{Deserialize, Serialize};
use shell_contract::gravity::GravityVector;
use thiserror::Error;

/// Настройки уровня.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelConfig {
    /// Угол между гравитацией и нормалью экрана, меньше которого телефон считается «плашмя», градусы.
    /// `[estimate]` 20° — подгонка, не Apple (Apple порог не публикует).
    pub flat_threshold_degrees: f32,
}

impl Default for LevelConfig {
    fn default() -> Self {
        Self {
            flat_threshold_degrees: 20.0,
        }
    }
}

/// Ошибка конфига уровня.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
#[error("порог «плашмя» должен быть в (0, 90) градусах, получено {0}")]
pub struct LevelConfigError(pub f32);

impl LevelConfig {
    pub fn validate(&self) -> Result<(), LevelConfigError> {
        if self.flat_threshold_degrees.is_finite()
            && self.flat_threshold_degrees > 0.0
            && self.flat_threshold_degrees < 90.0
        {
            Ok(())
        } else {
            Err(LevelConfigError(self.flat_threshold_degrees))
        }
    }
}

/// Что показать уровню.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LevelReading {
    /// Показаний ещё не было или вектор негоден (нулевой, NaN): уровень не рисуем.
    #[default]
    Unknown,
    /// Телефон плашмя: уровень не показывается.
    Flat,
    /// Телефон стоит: крен и отклонение от ближайшей оси.
    Tilted {
        /// Крен по часовой, градусы, `(−180°, 180°]`.
        roll_clockwise_degrees: f32,
        /// Отклонение от ближайшей оси, `(−45°, 45°]`: 0 — горизонт ровный.
        deviation_degrees: f32,
    },
}

/// Чистая функция: вектор гравитации → показание уровня.
pub fn level_reading(gravity: GravityVector, config: &LevelConfig) -> LevelReading {
    let GravityVector { x, y, z } = gravity;
    let magnitude = (x * x + y * y + z * z).sqrt();
    if !magnitude.is_finite() || magnitude <= f32::EPSILON {
        return LevelReading::Unknown;
    }
    // Угол между вектором и нормалью экрана (осью z): 0° — лежит экраном вверх, 180° — вниз.
    let angle_to_screen_normal = (z.abs() / magnitude).clamp(0.0, 1.0).acos().to_degrees();
    if angle_to_screen_normal < config.flat_threshold_degrees {
        return LevelReading::Flat;
    }
    let roll = (-x).atan2(y).to_degrees();
    LevelReading::Tilted {
        roll_clockwise_degrees: roll,
        deviation_degrees: deviation_from_nearest_axis(roll),
    }
}

/// Отклонение крена от ближайшей оси, `(−45°, 45°]`.
fn deviation_from_nearest_axis(roll_degrees: f32) -> f32 {
    // Остаток по модулю 90, смещённый в (−45, 45]. rem_euclid даёт [0, 90).
    let remainder = roll_degrees.rem_euclid(90.0);
    if remainder > 45.0 { remainder - 90.0 } else { remainder }
}
