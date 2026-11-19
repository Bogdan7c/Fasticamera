//! Размеры вида шкалы K3: риски и отметка текущего значения (`view::scale_items`).
//!
//! Всё `[estimate]` — подгонка, не Apple: в замерах (`layout.md` 2.2) размеры рисок панели Exposure
//! не измерены. Числа перенесены из прежних значений экрана камеры без изменения вида.

use serde::{Deserialize, Serialize};

use super::elements::positive;
use super::error::TokenError;

/// Размеры рисок шкалы, pt. Длина — поперёк оси шкалы, толщина — вдоль неё.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScaleTickTokens {
    /// Толщина риски (вдоль оси шкалы). `[estimate]` — подгонка, не Apple.
    pub thickness_pt: f64,
    /// Длина обычной риски ([`crate::value::Emphasis::Tick`]). `[estimate]` — подгонка, не Apple.
    pub tick_length_pt: f64,
    /// Длина крупной риски ([`crate::value::Emphasis::Reference`]): ноль, опорное значение.
    /// `[estimate]` — подгонка, не Apple.
    pub reference_length_pt: f64,
    /// Длина неподвижной отметки текущего значения в центре шкалы (жёлтая, длиннее крупной риски).
    /// `[estimate]` — подгонка, не Apple.
    pub current_mark_length_pt: f64,
}

impl Default for ScaleTickTokens {
    fn default() -> Self {
        Self {
            thickness_pt: 1.5,
            tick_length_pt: 10.0,
            reference_length_pt: 18.0,
            current_mark_length_pt: 23.4,
        }
    }
}

impl ScaleTickTokens {
    pub fn validate(&self) -> Result<(), TokenError> {
        positive("scale_ticks.thickness_pt", self.thickness_pt)?;
        positive("scale_ticks.tick_length_pt", self.tick_length_pt)?;
        positive("scale_ticks.reference_length_pt", self.reference_length_pt)?;
        positive("scale_ticks.current_mark_length_pt", self.current_mark_length_pt)
    }
}
