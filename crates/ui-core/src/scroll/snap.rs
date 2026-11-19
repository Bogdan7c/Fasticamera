//! Выбор точки прилипания (`motion.md` раздел 2, WWDC18 803).

use serde::{Deserialize, Serialize};

/// Политика выбора точки после броска.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapSelection {
    /// Ближайшая к проекции броска (колесо зума, шкалы).
    #[default]
    NearestToProjection,
    /// Только соседняя: не дальше ±1 шага от точки, с которой начали тянуть (карусель режимов
    /// листается по одному режиму, как бы сильно ни бросили).
    AdjacentOnly,
}

/// Выбранная точка: индекс в отсортированном списке и её смещение.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapChoice {
    pub index: usize,
    pub offset: f64,
}

/// Индекс ближайшей точки к `position` в отсортированном списке; при равенстве — меньший индекс.
pub fn nearest_index(points: &[f64], position: f64) -> Option<usize> {
    nearest_in_range(points, position, 0, points.len().checked_sub(1)?)
}

fn nearest_in_range(points: &[f64], position: f64, first: usize, last: usize) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for index in first..=last {
        let distance = (points.get(index)? - position).abs();
        // Строгое «<»: при равенстве остаётся меньший индекс (детерминизм).
        if best.is_none_or(|(_, best_distance)| distance < best_distance) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

/// Выбрать точку для проекции `projection`. `origin_index` — точка, с которой начали тянуть
/// (для [`SnapSelection::AdjacentOnly`]; без неё политика ведёт себя как «ближайшая»).
pub fn choose_snap_point(
    points: &[f64],
    projection: f64,
    origin_index: Option<usize>,
    selection: SnapSelection,
) -> Option<SnapChoice> {
    let last = points.len().checked_sub(1)?;
    let index = match (selection, origin_index) {
        (SnapSelection::AdjacentOnly, Some(origin)) => {
            let origin = origin.min(last);
            nearest_in_range(points, projection, origin.saturating_sub(1), (origin + 1).min(last))?
        }
        (SnapSelection::AdjacentOnly, None) | (SnapSelection::NearestToProjection, _) => {
            nearest_in_range(points, projection, 0, last)?
        }
    };
    Some(SnapChoice {
        index,
        offset: *points.get(index)?,
    })
}
