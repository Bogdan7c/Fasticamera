//! Где сетка статистики лежит в осях активной области сенсора (для весов замера AE).
//!
//! Регион тапа (`MeteringRegion`) задан в нормированных осях **активной области** сенсора — тех же, в
//! которых HAL понимает `AE_REGIONS`. Статистика ISP считается по другому прямоугольнику: по
//! кадрированию видоискателя (`ViewfinderGeometry::crop`) внутри полезной области буфера. Чтобы вес
//! плитки отражал именно тот кусок сцены, на который показал пользователь, нужен перевод
//! «кадрирование в буфере → прямоугольник в активной области».
//!
//! # Честное приближение
//! Паспорт потока не хранит размер активной области, поэтому берём **размер буфера** (на основной камере
//! OnePlus 11 буфер 4096×3072 совпадает с активной областью 4:3). Режим 16:9 читает центральную полосу
//! сенсора: данные лежат в строках 0–2303 буфера 4096×3072 (00 C2), а в осях активной области это строки
//! 384–2687. Поэтому для `SixteenByNine` полезная область считается **центрированной** в активной
//! области, для `FourByThree` — занимающей её целиком. Если у другой камеры вырезка не центральная,
//! тап сместится по оси вырезки; контракт паспорта для точного перевода нужно расширять отдельным
//! решением (в отчёте сессии 08a это отмечено).

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::stream::ReadoutAspect;
use scene_estimation::exposure::{MeteringWeightsError, TilePlacement};

/// Положение сетки статистики для кадрирования `crop` (в координатах буфера) потока `stream`.
pub(super) fn tile_placement(stream: &StreamPassport, crop: PixelRect) -> Result<TilePlacement, MeteringWeightsError> {
    let active = stream.buffer_size;
    if active.width == 0 || active.height == 0 {
        return Err(MeteringWeightsError::InvalidPlacement);
    }
    let (offset_x, offset_y) = valid_area_offset(stream.readout, active, stream.valid_area.size);
    let active_width = f64::from(active.width);
    let active_height = f64::from(active.height);
    let left = (f64::from(offset_x) + f64::from(crop.left)) / active_width;
    let top = (f64::from(offset_y) + f64::from(crop.top)) / active_height;
    let right = left + f64::from(crop.size.width) / active_width;
    let bottom = top + f64::from(crop.size.height) / active_height;
    TilePlacement::new(
        left as f32,
        top as f32,
        right as f32,
        bottom as f32,
        (active_width / active_height) as f32,
    )
}

/// Смещение начала буфера в осях активной области, пиксели.
fn valid_area_offset(readout: ReadoutAspect, active: PixelSize, valid: PixelSize) -> (u32, u32) {
    match readout {
        ReadoutAspect::FourByThree => (0, 0),
        ReadoutAspect::SixteenByNine => (
            active.width.saturating_sub(valid.width) / 2,
            active.height.saturating_sub(valid.height) / 2,
        ),
    }
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
