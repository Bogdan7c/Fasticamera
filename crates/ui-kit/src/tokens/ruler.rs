//! Размеры и яркость вида линейки шкалы (`view::ruler_items`): риски, текущая отметка, затухание у краёв.
//!
//! Линейка EV в нижней карточке iOS 27. Метки: `[measured: R00 t=…]` — с записи iOS 26
//! (3 px = 1 pt, решение 102), `[derived]` — выведено по яркости записи, `[estimate]` — подгонка.
//! Что такое «крупная» риска (каждая 3-я, целый EV), токены не знают: период задаёт вызывающий.

use serde::{Deserialize, Serialize};

use super::elements::positive;
use super::error::TokenError;

/// Вид линейки.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RulerTokens {
    /// Толщина риски (вдоль оси шкалы). `[measured: R00 t=35.50]`: 2,0.
    pub tick_thickness_pt: f64,
    /// Длина риски (поперёк оси), одинаковая у всех. `[measured: R00 t=35.50]`: 13,0.
    pub tick_length_pt: f64,
    /// Толщина текущей отметки. `[measured: R00 t=35.50]`: 2,0.
    pub current_mark_thickness_pt: f64,
    /// Длина текущей отметки (жёлтая). `[measured: R00 t=35.50]`: 26,0.
    pub current_mark_length_pt: f64,
    /// Альфа крупной риски (целый EV). `[derived]` по яркости R00 t=35.50: 0,8.
    pub major_tick_alpha: f32,
    /// Альфа мелкой риски. `[derived]` по яркости R00 t=35.50: 0,5.
    pub minor_tick_alpha: f32,
    /// До какого расстояния от текущей отметки (в pt) риски видны в полную силу.
    /// `[measured: R00 t=159.0]`: 4 шага линейки Exposure по 20 pt = 80. Затухание задано в pt, а не в шагах:
    /// линейка с другой плотностью (10 pt на риску) показывает те же ±120 pt, а не «столько-то рисок».
    pub fade_full_until_pt: f64,
    /// Длина одного звена спада яркости, pt. `[measured: R00 t=159.0]`: шаг Exposure, 20.
    pub fade_link_length_pt: f64,
    /// Доля яркости на расстоянии `full + 1` звено. `[measured: R00 t=159.0]`: 0,6 (5-й шаг).
    pub fade_first_step_factor: f32,
    /// Доля яркости на расстоянии `full + 2` звена; дальше риски не рисуются.
    /// `[measured: R00 t=159.0]`: 0,1 (6-й шаг).
    pub fade_second_step_factor: f32,
}

impl Default for RulerTokens {
    fn default() -> Self {
        Self {
            tick_thickness_pt: 2.0,
            tick_length_pt: 13.0,
            current_mark_thickness_pt: 2.0,
            current_mark_length_pt: 26.0,
            major_tick_alpha: 0.8,
            minor_tick_alpha: 0.5,
            fade_full_until_pt: 80.0,
            fade_link_length_pt: 20.0,
            fade_first_step_factor: 0.6,
            fade_second_step_factor: 0.1,
        }
    }
}

impl RulerTokens {
    /// Яркость риски по расстоянию до текущей отметки, pt: 1 до `fade_full_until_pt`, затем ломаная
    /// 1 → 0,6 → 0,1 (звено — `fade_link_length_pt`), дальше 0 — риска не рисуется.
    pub fn fade_factor(&self, distance_pt: f64) -> f32 {
        let past_full = (distance_pt.abs() - self.fade_full_until_pt) / self.fade_link_length_pt;
        let knots = [1.0, self.fade_first_step_factor, self.fade_second_step_factor];
        if past_full <= 0.0 {
            return knots[0];
        }
        let last_knot = knots.len() - 1;
        if past_full > last_knot as f64 {
            return 0.0;
        }
        // Звено ломаной и доля внутри него; последняя узловая точка принадлежит последнему звену.
        let segment = (past_full.floor() as usize).min(last_knot - 1);
        let within = (past_full - segment as f64) as f32;
        knots[segment] + (knots[segment + 1] - knots[segment]) * within
    }

    pub fn validate(&self) -> Result<(), TokenError> {
        positive("ruler.tick_thickness_pt", self.tick_thickness_pt)?;
        positive("ruler.tick_length_pt", self.tick_length_pt)?;
        positive("ruler.current_mark_thickness_pt", self.current_mark_thickness_pt)?;
        positive("ruler.current_mark_length_pt", self.current_mark_length_pt)?;
        positive("ruler.fade_full_until_pt", self.fade_full_until_pt)?;
        positive("ruler.fade_link_length_pt", self.fade_link_length_pt)?;
        for (name, value) in [
            ("ruler.major_tick_alpha", self.major_tick_alpha),
            ("ruler.minor_tick_alpha", self.minor_tick_alpha),
            ("ruler.fade_first_step_factor", self.fade_first_step_factor),
            ("ruler.fade_second_step_factor", self.fade_second_step_factor),
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
