//! Описание шкалы: поведение и вид без чисел (числа — в токенах).

use serde::{Deserialize, Serialize};

use crate::drive::{DragOrigin, GrowthDirection};
use crate::tokens::ScaleDensity;

/// Как шкала прилипает после броска.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapPolicy {
    /// К каждому тику (EV, ряды значений): значение всегда на шаге.
    #[default]
    EveryStep,
    /// Остановка где угодно, но возле опорного значения шкала «примагничивается» к нему
    /// (зум: упоры 0,5 / 1× / 2). Радиус магнита — токен плотности. Только для плавных моделей.
    ContinuousWithMagnets,
}

/// Рисуются ли риски. Свойство описания вида: механика (прилипание, события, хаптика) не меняется.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TickVisibility {
    #[default]
    Visible,
    /// Шкала без рисок («солнце» у рамки: тот же элемент, ничего не нарисовано).
    Hidden,
}

/// Описание шкалы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScaleSpec {
    /// Ось и знак: в какую сторону идёт палец, чтобы значение росло.
    pub direction: GrowthDirection,
    pub snap: SnapPolicy,
    /// Сколько pt занимает единица модели (токен).
    pub density: ScaleDensity,
    pub ticks: TickVisibility,
    /// От какой точки считать смещение пальца.
    pub drag_origin: DragOrigin,
}

impl Default for ScaleSpec {
    fn default() -> Self {
        Self {
            direction: GrowthDirection::FingerLeft,
            snap: SnapPolicy::EveryStep,
            density: ScaleDensity::Regular,
            ticks: TickVisibility::Visible,
            drag_origin: DragOrigin::FromRecognition,
        }
    }
}
