//! Данные карусели: элементы списком, раскладка по центрам.

use serde::{Deserialize, Serialize};

use super::error::CarouselError;
use crate::drive::{DragOrigin, GrowthDirection};

/// Элемент карусели: значение `T` (оно же идентичность) и ширина, pt. Смысл `T` карусель не знает:
/// это может быть режим съёмки, стиль, фокусное.
#[derive(Debug, Clone, PartialEq)]
pub struct CarouselEntry<T> {
    pub item: T,
    /// Ширина элемента вдоль оси, pt; конечная и > 0.
    pub width_points: f64,
}

/// Описание карусели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CarouselSpec {
    /// В какую сторону идёт палец, чтобы выбрался следующий элемент списка.
    pub direction: GrowthDirection,
    /// От какой точки считать смещение пальца.
    pub drag_origin: DragOrigin,
}

impl Default for CarouselSpec {
    fn default() -> Self {
        Self {
            direction: GrowthDirection::FingerLeft,
            drag_origin: DragOrigin::FromRecognition,
        }
    }
}

/// Где элемент стоит сейчас — для рисования и попадания пальца.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemPlacement {
    pub index: usize,
    /// Центр элемента относительно центра видимой области, pt; положительное — в сторону
    /// следующих элементов списка.
    pub center_offset_points: f64,
    pub width_points: f64,
}

/// Смещения точек прилипания: центр каждого элемента минус центр первого. Элементы идут подряд
/// через промежуток `gap_points`.
pub(super) fn snap_offsets<T>(entries: &[CarouselEntry<T>], gap_points: f64) -> Result<Vec<f64>, CarouselError> {
    if entries.is_empty() {
        return Err(CarouselError::EmptyEntries);
    }
    let mut centers = Vec::with_capacity(entries.len());
    let mut leading_edge = 0.0;
    for (index, entry) in entries.iter().enumerate() {
        if !entry.width_points.is_finite() || entry.width_points <= 0.0 {
            return Err(CarouselError::InvalidWidth {
                index,
                width_points: entry.width_points,
            });
        }
        centers.push(leading_edge + entry.width_points * 0.5);
        leading_edge += entry.width_points + gap_points;
    }
    let first_center = centers[0];
    Ok(centers.into_iter().map(|center| center - first_center).collect())
}

/// Движение карусели в один момент: положение и скорость контента вдоль оси капсулы. Это то, что один дом
/// карусели отдаёт другому при передаче эстафеты ([`SnapCarousel::adopt_motion`]): приёмник продолжает
/// рисовать ровно с того места и с той скоростью, где остановился передающий.
///
/// [`SnapCarousel::adopt_motion`]: super::SnapCarousel::adopt_motion
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarouselMotion {
    /// Смещение контента, pt (0 — первый элемент по центру).
    pub offset_points: f64,
    /// Скорость контента, pt/с.
    pub velocity_points_per_second: f64,
}
