//! Размеры и ведение «точки в круге» (`disc_pad::DiscPad`, вид `view::disc_pad_items`).
//!
//! Элемент придуман для колёс цвета раздела калибровки (14b2): эталона Apple нет, поэтому все числа
//! `[estimate]` — подгонка, не Apple. Токен о цвете не знает: так выглядит любой «джойстик-круг».

use serde::{Deserialize, Serialize};

use super::elements::positive;
use super::error::TokenError;

/// Ведение и вид площадки «точка в круге».
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DiscPadTokens {
    /// Сколько pt точки на 1 pt пальца (относительное ведение с замедлением, решение 206: 2 pt пальца = 1 pt
    /// точки). `[estimate]` 0,5 — подгонка, не Apple.
    pub drive_ratio: f64,
    /// Диаметр точки-шайбы. `[estimate]` 28.
    pub puck_diameter_pt: f64,
    /// Толщина обводки шайбы. `[estimate]` 2.
    pub puck_stroke_pt: f64,
    /// Прозрачность заливки шайбы (обводка — полным цветом текста). `[estimate]` 0,25.
    pub puck_fill_alpha: f32,
    /// Длина каждой из двух линий перекрестия центра (целиком, от края до края). `[estimate]` 14.
    pub crosshair_length_pt: f64,
    /// Толщина линий перекрестия. `[estimate]` 1.
    pub crosshair_thickness_pt: f64,
    /// Прозрачность перекрестия. `[estimate]` 0,6.
    pub crosshair_alpha: f32,
}

impl Default for DiscPadTokens {
    fn default() -> Self {
        Self {
            drive_ratio: 0.5,
            puck_diameter_pt: 28.0,
            puck_stroke_pt: 2.0,
            puck_fill_alpha: 0.25,
            crosshair_length_pt: 14.0,
            crosshair_thickness_pt: 1.0,
            crosshair_alpha: 0.6,
        }
    }
}

impl DiscPadTokens {
    /// Размеры конечны и > 0, прозрачности в 0..1.
    pub fn validate(&self) -> Result<(), TokenError> {
        for (name, value) in [
            ("disc_pad.drive_ratio", self.drive_ratio),
            ("disc_pad.puck_diameter_pt", self.puck_diameter_pt),
            ("disc_pad.puck_stroke_pt", self.puck_stroke_pt),
            ("disc_pad.crosshair_length_pt", self.crosshair_length_pt),
            ("disc_pad.crosshair_thickness_pt", self.crosshair_thickness_pt),
        ] {
            positive(name, value)?;
        }
        for (name, value) in [
            ("disc_pad.puck_fill_alpha", self.puck_fill_alpha),
            ("disc_pad.crosshair_alpha", self.crosshair_alpha),
        ] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(TokenError::OutOfRange {
                    name,
                    value: f64::from(value),
                });
            }
        }
        Ok(())
    }
}
