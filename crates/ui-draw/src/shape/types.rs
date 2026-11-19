//! Описание фигур: размеры и скругление.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::geometry::PxSize;

/// Показатель суперэллипса по умолчанию для «непрерывного» скругления iOS.
///
/// `[reverse]`: часто приводимая оценка «squircle iOS ≈ n 5». Подгонка, не Apple — Apple форму
/// кривой не публикует. Проверять глазами на 11a.
pub const DEFAULT_CONTINUOUS_EXPONENT: f32 = 5.0;

/// Фигура задана неверно.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum ShapeError {
    /// Размер или радиус — не число либо отрицательный.
    #[error("размер или радиус фигуры «{field}» некорректен: {value}")]
    InvalidDimension { field: &'static str, value: f32 },
    /// Показатель суперэллипса меньше 2 (меньше — скругление «вогнутее» окружности) или не число.
    #[error("показатель непрерывного скругления должен быть конечным и ≥ 2, получено {0}")]
    InvalidExponent(f32),
}

/// Кривая угла.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CornerCurve {
    /// Угол — суперэллипс `|x/r|^n + |y/r|^n = 1` (`cornerCurve = .continuous` у iOS). Кривизна на
    /// стыке с прямой стороной плавно растёт от нуля, а не скачет до `1/r`, как у дуги.
    Continuous { exponent: f32 },
    /// Обычная дуга окружности (`n = 2`).
    Circular,
}

/// Скругление угла: радиус и кривая.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CornerSpec {
    /// Запрошенный радиус, px. Фактический ограничен половиной меньшей стороны —
    /// см. [`CornerSpec::effective_radius_px`].
    pub radius_px: f32,
    pub curve: CornerCurve,
}

impl CornerSpec {
    /// Непрерывное скругление iOS с показателем по умолчанию.
    pub const fn continuous(radius_px: f32) -> Self {
        Self {
            radius_px,
            curve: CornerCurve::Continuous {
                exponent: DEFAULT_CONTINUOUS_EXPONENT,
            },
        }
    }

    /// Скругление дугой окружности.
    pub const fn circular(radius_px: f32) -> Self {
        Self {
            radius_px,
            curve: CornerCurve::Circular,
        }
    }

    /// Показатель нормы в углу: 2 для дуги.
    pub fn exponent(&self) -> f32 {
        match self.curve {
            CornerCurve::Continuous { exponent } => exponent,
            CornerCurve::Circular => 2.0,
        }
    }

    /// Фактический радиус: не больше половины меньшей стороны (иначе скругления соседних углов
    /// перекрылись бы и форма перестала быть выпуклой) и не меньше нуля. Это явное, а не молчаливое
    /// ограничение: капсула — это скругление с радиусом ровно `min(w,h)/2`.
    pub fn effective_radius_px(&self, size: PxSize) -> f32 {
        self.radius_px.clamp(0.0, size.min_side() * 0.5)
    }
}

/// Геометрия фигуры в локальных координатах: центр в (0,0), ось y вниз.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeGeometry {
    /// Прямоугольник со скруглёнными углами.
    RoundedRect { size: PxSize, corner: CornerSpec },
    /// Капсула: скругление дугой радиусом в половину меньшей стороны (форма стекла по умолчанию у Apple).
    Capsule { size: PxSize },
    /// Круг.
    Circle { diameter: f32 },
}

impl ShapeGeometry {
    /// Размер описывающего прямоугольника.
    pub fn extent(&self) -> PxSize {
        match *self {
            ShapeGeometry::RoundedRect { size, .. } | ShapeGeometry::Capsule { size } => size,
            ShapeGeometry::Circle { diameter } => PxSize::new(diameter, diameter),
        }
    }

    /// Параметры SDF «скруглённого ящика», общие для всех трёх фигур: половина размера, радиус,
    /// показатель. Единственное место, где фигура превращается в числа для формулы расстояния:
    /// эталон ([`signed_distance`](crate::shape::signed_distance)) и упаковка рендера берут их отсюда.
    pub fn sdf_parameters(&self) -> SdfParameters {
        let size = self.extent();
        let half_size = PxSize::new(size.width * 0.5, size.height * 0.5);
        match *self {
            ShapeGeometry::RoundedRect { corner, .. } => SdfParameters {
                half_size,
                radius: corner.effective_radius_px(size),
                exponent: corner.exponent(),
            },
            // Капсула и круг — предельные случаи: радиус равен половине меньшей стороны.
            ShapeGeometry::Capsule { .. } | ShapeGeometry::Circle { .. } => SdfParameters {
                half_size,
                radius: size.min_side() * 0.5,
                exponent: 2.0,
            },
        }
    }

    /// Проверить размеры, радиус и показатель.
    pub fn validate(&self) -> Result<(), ShapeError> {
        let size = self.extent();
        check_dimension("width", size.width)?;
        check_dimension("height", size.height)?;
        if let ShapeGeometry::RoundedRect { corner, .. } = *self {
            check_dimension("radius", corner.radius_px)?;
            let exponent = corner.exponent();
            if !exponent.is_finite() || exponent < 2.0 {
                return Err(ShapeError::InvalidExponent(exponent));
            }
        }
        Ok(())
    }
}

fn check_dimension(field: &'static str, value: f32) -> Result<(), ShapeError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(ShapeError::InvalidDimension { field, value })
    }
}

/// Параметры SDF фигуры: `d = ‖max(q, 0)‖ₙ + min(max(q.x, q.y), 0) − radius`, `q = |p| − (half_size − radius)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdfParameters {
    /// Половина ширины и высоты описывающего прямоугольника, px.
    pub half_size: PxSize,
    /// Фактический радиус угла, px (уже ограничен половиной меньшей стороны).
    pub radius: f32,
    /// Показатель нормы в угловой зоне: 2 — дуга окружности, больше — суперэллипс.
    pub exponent: f32,
}
