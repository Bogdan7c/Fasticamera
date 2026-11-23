//! Значение площадки: точка в единичном круге.

use thiserror::Error;

/// Почему точку нельзя построить.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum DiscPointError {
    /// Координата — NaN или бесконечность: это ошибка источника, а не «за краем круга».
    #[error("координаты точки должны быть конечными, получено ({x}, {y})")]
    NotFinite { x: f64, y: f64 },
}

/// Точка в единичном круге. Оси «глазные», как у математики: `x` вправо, `y` ВВЕРХ.
///
/// Инвариант: `x² + y² <= 1` (строго, без допуска) — поля приватны, построить точку за кругом нельзя.
/// Центр круга — `(0, 0)`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DiscPoint {
    x: f64,
    y: f64,
}

impl DiscPoint {
    /// Центр круга.
    pub const CENTER: DiscPoint = DiscPoint { x: 0.0, y: 0.0 };

    /// Точка из координат; за кругом прижимается к краю под тем же углом (не отказ, решение 206).
    /// NaN и бесконечности — [`DiscPointError::NotFinite`].
    pub fn clamped(x: f64, y: f64) -> Result<Self, DiscPointError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(DiscPointError::NotFinite { x, y });
        }
        // Критерий инварианта один на всю структуру: `x² + y² <= 1`.
        if x * x + y * y <= 1.0 {
            return Ok(Self { x, y });
        }
        // Сначала делим на большую из координат (она ≠ 0, раз квадрат длины > 1): обе координаты в [−1, 1],
        // и длина не переполняется даже для огромных значений.
        let largest = x.abs().max(y.abs());
        let (unit_x, unit_y) = (x / largest, y / largest);
        let length = unit_x.hypot(unit_y);
        let mut edge_x = unit_x / length;
        let mut edge_y = unit_y / length;
        // Округление деления может оставить точку на ULP за кругом: поджимаем внутрь, пока критерий не выполнится.
        while edge_x * edge_x + edge_y * edge_y > 1.0 {
            edge_x *= 1.0 - f64::EPSILON;
            edge_y *= 1.0 - f64::EPSILON;
        }
        Ok(Self { x: edge_x, y: edge_y })
    }

    /// Вправо от центра, −1…1.
    pub fn x(self) -> f64 {
        self.x
    }

    /// ВВЕРХ от центра, −1…1.
    pub fn y(self) -> f64 {
        self.y
    }

    /// Расстояние от центра, 0…1.
    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}
