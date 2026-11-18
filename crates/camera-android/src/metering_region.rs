//! Перевод региона замера/фокуса из нейтральных координат в запись HAL (сессия 08).
//!
//! Движок задаёт регион как [`MeteringRegion`]: центр в долях активной области сенсора и сторона
//! как доля **меньшей** стороны этой области. HAL ждёт `int32[5]`: `(xmin, ymin, xmax, ymax, weight)`
//! в пикселях активной области, где `(0, 0)` — её левый верхний угол, `xmin/ymin` включительно,
//! а `xmax/ymax` **исключительно** (заголовок NDK, `ACAMERA_CONTROL_AE_REGIONS`). Поэтому смещение
//! `active_array.left/top` не добавляется, а полнокадровый регион кончается на `width`, не `width - 1`.
//!
//! Регион у края **обрезается** по границе, а не сдвигается: центр тапа остаётся центром замера.
//! Функция чистая: ни NDK, ни логирования.

use camera_contract::geometry::PixelSize;
use camera_contract::three_a::MeteringRegion;

/// Вес региона. Диапазон HAL — `[0, 1000]`, вес относителен другим регионам; регион у нас всегда
/// один, поэтому берём максимум: любое ненулевое значение дало бы тот же эффект.
pub const METERING_WEIGHT_MAX: i32 = 1000;

/// Сколько чисел занимает один регион в записи HAL.
pub const REGION_ENTRY_LEN: usize = 5;

/// Запись региона для HAL: `[xmin, ymin, xmax, ymax, weight]`.
pub type RegionEntry = [i32; REGION_ENTRY_LEN];

/// Регион в координатах активной области. Результат всегда внутри `[0, size]` и не меньше
/// одного пикселя по каждой оси; `None` — активная область вырождена (нулевая сторона) и
/// корректного региона построить нельзя.
pub fn region_to_active_array(region: MeteringRegion, active_array: PixelSize) -> Option<RegionEntry> {
    let (x_min, x_max) = axis_span(region.center().x(), region, active_array.width, active_array)?;
    let (y_min, y_max) = axis_span(region.center().y(), region, active_array.height, active_array)?;
    Some([x_min, y_min, x_max, y_max, METERING_WEIGHT_MAX])
}

/// Отрезок региона по одной оси: `[min, max)` в пикселях, обрезанный по `[0, axis_length]`.
fn axis_span(
    center_fraction: f32,
    region: MeteringRegion,
    axis_length: u32,
    active_array: PixelSize,
) -> Option<(i32, i32)> {
    let shorter_side = active_array.width.min(active_array.height);
    if axis_length == 0 || shorter_side == 0 {
        return None;
    }
    // Сторона квадрата в пикселях: не меньше одного пикселя, иначе HAL получит пустой регион.
    let side = (f64::from(region.size_fraction()) * f64::from(shorter_side))
        .round()
        .max(1.0);
    let center = f64::from(center_fraction) * f64::from(axis_length);
    let unclipped_min = (center - side / 2.0).round();
    let unclipped_max = unclipped_min + side;
    let length = f64::from(axis_length);
    // Начало не дальше последнего пикселя, чтобы после обрезки остался хотя бы один пиксель.
    let min = unclipped_min.clamp(0.0, length - 1.0);
    let max = unclipped_max.clamp(min + 1.0, length);
    Some((to_i32(min), to_i32(max)))
}

/// Значение уже внутри `[0, axis_length]`, поэтому усечение невозможно; `try_from` на крайний
/// случай насыщает, а не паникует.
fn to_i32(value: f64) -> i32 {
    // `as` из f64 в i64 насыщает и не паникует; значение здесь неотрицательное и целое.
    i32::try_from(value as i64).unwrap_or(i32::MAX)
}

#[cfg(test)]
#[path = "metering_region_tests.rs"]
mod tests;
