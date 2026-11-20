//! Кадрирование видоискателя: какое соотношение сторон и какой цифровой зум показать.
//!
//! Нейтральный тип (сессия 07): экран камеры (будущая сессия 13 — зум и форматы 4:3 / 16:9 / 1:1)
//! выбирает [`ViewfinderCrop`], ISP видоискателя получает уже разрешённый прямоугольник
//! ([`ViewfinderCrop::resolve`]) и ничего не знает о кнопках интерфейса.
//!
//! # Правила разрешения
//! 1. Соотношение сторон не зависит от ориентации: `long:short` кладётся длинной стороной вдоль
//!    длинной стороны полезной области сенсора (4:3 на сенсоре 4096×3072 — вся область; 16:9 —
//!    полоса по высоте; 1:1 — квадрат по короткой стороне).
//! 2. Сначала берётся НАИБОЛЬШИЙ прямоугольник нужного соотношения внутри полезной области, затем он
//!    уменьшается в `zoom` раз по каждой оси (зум 2× — вдвое уже и вдвое ниже).
//! 3. Прямоугольник ставится центром в [`CropCenter`]: по умолчанию в центр полезной области, при
//!    зуме — на оптическую ось камеры (сессия 13, выравнивание соседних камер). Если прямоугольник
//!    с таким центром выходит за полезную область, он сдвигается внутрь (не уменьшается).
//! 4. Все края и размеры выравниваются до ЧЁТНЫХ (вниз): ISP работает ячейками CFA 2×2, и ячейка,
//!    разрезанная краем, перепутала бы цвета каналов. Поэтому результат всегда лежит внутри
//!    полезной области и начинается с той же позиции CFA, что и она.

use serde::Serialize;

use crate::geometry::{PixelRect, PixelSize};

/// Соотношение сторон кадрирования.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CropAspect {
    /// Как у полезной области сенсора (на основной камере OnePlus 11 — 4:3 или 16:9 по режиму).
    SensorNative,
    /// Длинная сторона : короткая (4:3, 16:9, 1:1). Ориентацию задаёт сенсор, а не это значение.
    LongToShort {
        /// Длинная сторона, условные единицы (> 0, ≥ `short`).
        long: u32,
        /// Короткая сторона, условные единицы (> 0).
        short: u32,
    },
}

impl CropAspect {
    /// 4:3 — основной формат фото.
    pub const FOUR_BY_THREE: Self = Self::LongToShort { long: 4, short: 3 };
    /// 16:9 — видео.
    pub const SIXTEEN_BY_NINE: Self = Self::LongToShort { long: 16, short: 9 };
    /// 1:1 — квадрат.
    pub const SQUARE: Self = Self::LongToShort { long: 1, short: 1 };
}

/// Цифровой зум: во сколько раз уменьшить кадрирование по каждой оси. Всегда конечен и ≥ 1
/// (меньше 1 — это «показать больше, чем есть на сенсоре», такого не бывает).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ZoomFactor(f32);

/// Значение зума вне допустимого диапазона.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("зум {value} должен быть конечным и не меньше 1")]
pub struct InvalidZoomFactor {
    /// Отклонённое значение.
    pub value: f32,
}

impl ZoomFactor {
    /// Без зума.
    pub const NONE: Self = Self(1.0);

    /// Проверенный зум; NaN, бесконечность и значения меньше 1 — ошибка.
    pub fn new(value: f32) -> Result<Self, InvalidZoomFactor> {
        if value.is_finite() && value >= 1.0 {
            Ok(Self(value))
        } else {
            Err(InvalidZoomFactor { value })
        }
    }

    /// Значение (≥ 1).
    pub fn value(self) -> f32 {
        self.0
    }
}

/// Кадрирование не разрешается для этой полезной области.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ViewfinderCropError {
    /// Соотношение `long:short` с нулём или с короткой стороной длиннее длинной.
    #[error("соотношение сторон {long}:{short}: нужно long ≥ short > 0")]
    InvalidAspect {
        /// Длинная сторона.
        long: u32,
        /// Короткая сторона.
        short: u32,
    },
    /// Полезная область не выровнена по ячейке CFA 2×2 или меньше одной ячейки.
    #[error("полезная область {area:?}: края и размеры должны быть чётными, размер ≥ 2×2")]
    ValidAreaNotCellAligned {
        /// Полученная область.
        area: PixelRect,
    },
    /// После соотношения сторон и зума не осталось ни одной целой ячейки 2×2.
    #[error("кадрирование {width}×{height} меньше одной ячейки CFA 2×2")]
    CropTooSmall {
        /// Ширина до выравнивания, пиксели.
        width: u64,
        /// Высота до выравнивания, пиксели.
        height: u64,
    },
}

/// Куда поставить центр кадрирования.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CropCenter {
    /// Центр полезной области (без калибровки и для форматов без зума).
    ValidAreaCenter,
    /// Точка в пикселях буфера RAW (пиксель буфера = пиксель активной области, см.
    /// `camera-screen::viewfinder_map`): оптическая ось камеры по калибровке HAL.
    BufferPixel {
        /// Горизонталь, пиксели (может быть дробной).
        x: f32,
        /// Вертикаль, пиксели.
        y: f32,
    },
}

/// Что показать видоискателем: соотношение сторон, зум и центр кадрирования.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ViewfinderCrop {
    /// Соотношение сторон.
    pub aspect: CropAspect,
    /// Цифровой зум.
    pub zoom: ZoomFactor,
    /// Центр кадрирования.
    pub center: CropCenter,
}

impl ViewfinderCrop {
    /// Вся полезная область без зума.
    pub const FULL_SENSOR: Self = Self::centered(CropAspect::SensorNative, ZoomFactor::NONE);

    /// Кадрирование по центру полезной области.
    pub const fn centered(aspect: CropAspect, zoom: ZoomFactor) -> Self {
        Self {
            aspect,
            zoom,
            center: CropCenter::ValidAreaCenter,
        }
    }

    /// Прямоугольник в пикселях буфера (см. правила модуля). Ошибки — до какой-либо работы GPU.
    pub fn resolve(&self, valid_area: PixelRect) -> Result<PixelRect, ViewfinderCropError> {
        check_cell_aligned(valid_area)?;
        let (width, height) = largest_rect_with_aspect(valid_area.size, self.aspect)?;
        // Зум делит обе стороны; f64 точно представляет любые u32 и даёт детерминированное округление вниз.
        let zoom = f64::from(self.zoom.value());
        let zoomed_width = (width as f64 / zoom).floor() as u64;
        let zoomed_height = (height as f64 / zoom).floor() as u64;
        let crop_width = round_down_to_even(zoomed_width);
        let crop_height = round_down_to_even(zoomed_height);
        if crop_width < 2 || crop_height < 2 {
            return Err(ViewfinderCropError::CropTooSmall {
                width: zoomed_width,
                height: zoomed_height,
            });
        }
        // Разности не отрицательны: кадрирование не больше полезной области по построению.
        let free_width = u64::from(valid_area.size.width) - crop_width;
        let free_height = u64::from(valid_area.size.height) - crop_height;
        let (left_margin, top_margin) = match self.center {
            CropCenter::ValidAreaCenter => (round_down_to_even(free_width / 2), round_down_to_even(free_height / 2)),
            CropCenter::BufferPixel { x, y } => (
                margin_toward(x, valid_area.left, crop_width, free_width),
                margin_toward(y, valid_area.top, crop_height, free_height),
            ),
        };
        // Все значения ≤ размеров полезной области (u32), сужение без потерь.
        Ok(PixelRect {
            left: valid_area.left + left_margin as u32,
            top: valid_area.top + top_margin as u32,
            size: PixelSize::new(crop_width as u32, crop_height as u32),
        })
    }
}

/// Полезная область обязана состоять из целых ячеек 2×2.
fn check_cell_aligned(area: PixelRect) -> Result<(), ViewfinderCropError> {
    let aligned = [area.left, area.top, area.size.width, area.size.height]
        .iter()
        .all(|value| value % 2 == 0);
    if !aligned || area.size.width < 2 || area.size.height < 2 {
        return Err(ViewfinderCropError::ValidAreaNotCellAligned { area });
    }
    Ok(())
}

/// Наибольший прямоугольник соотношения `aspect` внутри `size` (целые пиксели, округление вниз).
fn largest_rect_with_aspect(size: PixelSize, aspect: CropAspect) -> Result<(u64, u64), ViewfinderCropError> {
    let (width, height) = (u64::from(size.width), u64::from(size.height));
    let (long, short) = match aspect {
        CropAspect::SensorNative => return Ok((width, height)),
        CropAspect::LongToShort { long, short } => {
            if short == 0 || long < short {
                return Err(ViewfinderCropError::InvalidAspect { long, short });
            }
            (u64::from(long), u64::from(short))
        }
    };
    // Длинная сторона соотношения ложится вдоль длинной стороны области (ориентация — от сенсора).
    let (ratio_width, ratio_height) = if width >= height { (long, short) } else { (short, long) };
    // Сравнение w/h с ratio_width/ratio_height без деления: область «шире» нужного — режем ширину.
    if width * ratio_height >= height * ratio_width {
        Ok((height * ratio_width / ratio_height, height))
    } else {
        Ok((width, width * ratio_height / ratio_width))
    }
}

/// Отступ от края полезной области, при котором центр кадрирования ближе всего к `center_px`:
/// сдвиг внутрь, если центр у края (кадрирование не выходит за область), и выравнивание до чётного.
/// Нечисловой центр — как центр области (честного места нет, а падать на кадре нельзя).
fn margin_toward(center_px: f32, area_start: u32, crop_size: u64, free: u64) -> u64 {
    if !center_px.is_finite() {
        return round_down_to_even(free / 2);
    }
    let wanted = f64::from(center_px) - f64::from(area_start) - crop_size as f64 / 2.0;
    // clamp держит кадрирование внутри области; free ≤ u32, f64 точен.
    let clamped = wanted.round().clamp(0.0, free as f64) as u64;
    round_down_to_even(clamped)
}

fn round_down_to_even(value: u64) -> u64 {
    value & !1
}

#[cfg(test)]
#[path = "viewfinder_crop_tests.rs"]
mod tests;
