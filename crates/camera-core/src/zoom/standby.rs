//! Какие соседние камеры греть (13b): правило над шкалой зума, без слов «основная/ширик».
//!
//! Соседи текущего объектива — остальные камеры **того же направления** (у фронтальной их нет).
//! Порядок — по близости собственного коэффициента камеры к текущему коэффициенту экрана:
//! `|ln(native / ratio)|` (шкала мультипликативная: 0,5× и 2× одинаково далеки от 1×). Ничья
//! решается порядком шкалы (по возрастанию коэффициента). Результат обрезается по ёмкости.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{LensZoomPoint, ZoomCatalog, ZoomRatio};

/// Соседи объектива `current` направления `facing`, ближайшие к `ratio` первыми, не больше
/// `capacity`. Нет группы, нет такого объектива или `capacity == 0` — пусто.
pub fn standby_candidates(
    catalog: &ZoomCatalog,
    facing: LensFacing,
    current: LensRole,
    ratio: ZoomRatio,
    capacity: usize,
) -> Vec<&LensZoomPoint> {
    let Some(group) = catalog.group(facing) else {
        return Vec::new();
    };
    if group.lens_for_role(current).is_none() {
        return Vec::new();
    }
    let mut neighbours: Vec<&LensZoomPoint> = group.lenses.iter().filter(|lens| lens.role != current).collect();
    // Сортировка устойчива: при равном расстоянии остаётся порядок шкалы.
    neighbours.sort_by(|left, right| distance_on_scale(left, ratio).total_cmp(&distance_on_scale(right, ratio)));
    neighbours.truncate(capacity);
    neighbours
}

/// Расстояние от коэффициента экрана до собственного коэффициента камеры в логарифмической шкале.
fn distance_on_scale(lens: &LensZoomPoint, ratio: ZoomRatio) -> f32 {
    (lens.native_ratio.value() / ratio.value()).ln().abs()
}
