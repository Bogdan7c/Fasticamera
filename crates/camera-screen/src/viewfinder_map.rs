//! Перевод «точка на экране ↔ `SensorPoint`» в обе стороны.
//!
//! Нужен в двух местах: тап по кадру превращается в точку для камеры, а квадрат фокуса ставится по
//! точке, которую вернула камера (а не по последнему тапу: камера может двигать точку сама —
//! задел под фокус-трекинг, решение 69).
//!
//! # Цепочка преобразований (экран → сенсор)
//! 1. Точка в pt → доли кадра на экране `(u, v)`, `(0, 0)` — левый верхний угол прямого кадра.
//! 2. Зеркало фронтальной камеры (параметр-задел): `u → 1 − u`.
//! 3. Обратный поворот в оси сенсора. ISP поворачивает кадр сенсора по часовой на `turns` четвертей
//!    (`isp-vulkan/shaders/viewfinder_isp.glsl`, `cell_of_output`); берём ровно обратное:
//!
//!    | turns | сенсор `(sx, sy)` по прямому `(u, v)` |
//!    |-------|---------------------------------------|
//!    | 0     | `(u, v)`                              |
//!    | 1     | `(v, 1 − u)`                          |
//!    | 2     | `(1 − u, 1 − v)`                      |
//!    | 3     | `(1 − v, u)`                          |
//! 4. Доли внутри разрешённого кадрирования → пиксели буфера (`crop.left + sx × crop.width`).
//! 5. Пиксели → доли **активной области** сенсора, как их понимает `SensorPoint`
//!    (`camera-android::metering_region`: `(0, 0)` — левый верхний угол активной области, смещение
//!    `active_array.left/top` не добавляется).
//!
//! **Допущение**: пиксель буфера `(x, y)` совпадает с пикселем активной области `(x, y)` (буфер RAW
//! начинается в левом верхнем углу активной области, а `valid_area` лежит внутри буфера). Для
//! 16:9@60 (строки 0–2303 из 3072) это значит, что HAL считает регион в тех же пикселях буфера;
//! проверить на телефоне в сессии 12a.

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::three_a::{InvalidMeteringRegion, SensorPoint};
use camera_contract::viewfinder_crop::{ViewfinderCrop, ViewfinderCropError};
use thiserror::Error;
use ui_core::geometry::{Point, Rect};

/// Зеркалирование кадра по горизонтали на экране (селфи-режим «зеркальная фронталка»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MirrorMode {
    /// Без зеркала (задняя камера).
    #[default]
    None,
    /// Зеркало по горизонтали. В v1 фронталки нет — параметр-задел (сессии 13 и 16).
    Horizontal,
}

/// Геометрия потока камеры, нужная для перевода координат (приходит в `CameraFacts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamGeometry {
    /// Полезная область буфера (`StreamPassport::valid_area`), по ней считается кадрирование.
    pub valid_area: PixelRect,
    /// Размер активной области сенсора: относительно него нормируется `SensorPoint`.
    pub active_array_size: PixelSize,
    /// Ориентация сенсора.
    pub sensor_orientation: SensorOrientation,
    /// Зеркало на экране.
    pub mirror: MirrorMode,
}

/// Почему перевод невозможен.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum MappingError {
    #[error("кадрирование не разрешается для полезной области: {0}")]
    Crop(#[from] ViewfinderCropError),
    #[error("активная область сенсора {width}×{height} вырождена")]
    DegenerateActiveArray { width: u32, height: u32 },
    #[error("точка вне кадра")]
    OutsideFrame,
    #[error("точка сенсора вне кадрирования")]
    OutsideCrop,
    #[error("координата вне диапазона: {0}")]
    SensorPoint(#[from] InvalidMeteringRegion),
}

/// Перевод для конкретного кадра на экране: прямоугольник кадра + разрешённое кадрирование + поток.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewfinderMapping {
    frame: Rect,
    crop: PixelRect,
    active_array_size: PixelSize,
    quarter_turns: u32,
    mirror: MirrorMode,
}

impl ViewfinderMapping {
    /// Собирает перевод. Ошибка — кадрирование не разрешилось или активная область вырождена
    /// (ни одна точка не переводится: честный отказ, а не угадывание).
    pub fn new(frame: Rect, crop: &ViewfinderCrop, stream: &StreamGeometry) -> Result<Self, MappingError> {
        if stream.active_array_size.width == 0 || stream.active_array_size.height == 0 {
            return Err(MappingError::DegenerateActiveArray {
                width: stream.active_array_size.width,
                height: stream.active_array_size.height,
            });
        }
        Ok(Self {
            frame,
            crop: crop.resolve(stream.valid_area)?,
            active_array_size: stream.active_array_size,
            quarter_turns: stream.sensor_orientation.quarter_turns_clockwise(),
            mirror: stream.mirror,
        })
    }

    /// Разрешённое кадрирование в пикселях буфера (то, что получит ISP).
    pub fn crop_rect(&self) -> PixelRect {
        self.crop
    }

    /// Точка экрана (pt) → точка сенсора. Вне кадра — [`MappingError::OutsideFrame`] (тап по чёрной
    /// полосе фокус не ставит).
    pub fn screen_to_sensor(&self, point: Point) -> Result<SensorPoint, MappingError> {
        let u = (point.x - self.frame.min_x()) / self.frame.size.width;
        let v = (point.y - self.frame.min_y()) / self.frame.size.height;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return Err(MappingError::OutsideFrame);
        }
        let u = if self.mirror == MirrorMode::Horizontal {
            1.0 - u
        } else {
            u
        };
        let (crop_x, crop_y) = match self.quarter_turns {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        };
        let buffer_x = f64::from(self.crop.left) + crop_x * f64::from(self.crop.size.width);
        let buffer_y = f64::from(self.crop.top) + crop_y * f64::from(self.crop.size.height);
        // clamp только гасит ошибку округления f64 на самом краю; настоящий выход за [0, 1] невозможен.
        let x = (buffer_x / f64::from(self.active_array_size.width)).clamp(0.0, 1.0);
        let y = (buffer_y / f64::from(self.active_array_size.height)).clamp(0.0, 1.0);
        Ok(SensorPoint::new(x as f32, y as f32)?)
    }

    /// Точка сенсора → точка экрана (pt). Точка вне кадрирования — [`MappingError::OutsideCrop`]
    /// (квадрат фокуса не рисуется за пределами видимого кадра).
    pub fn sensor_to_screen(&self, point: SensorPoint) -> Result<Point, MappingError> {
        let buffer_x = f64::from(point.x()) * f64::from(self.active_array_size.width);
        let buffer_y = f64::from(point.y()) * f64::from(self.active_array_size.height);
        // Допуск в полпикселя буфера: SensorPoint хранит f32, на краю кадрирования он может
        // «недотянуть» до границы на ничтожную долю.
        let crop_x = (buffer_x - f64::from(self.crop.left)) / f64::from(self.crop.size.width);
        let crop_y = (buffer_y - f64::from(self.crop.top)) / f64::from(self.crop.size.height);
        let tolerance = 0.5 / f64::from(self.crop.size.width.min(self.crop.size.height));
        let range = -tolerance..=1.0 + tolerance;
        if !range.contains(&crop_x) || !range.contains(&crop_y) {
            return Err(MappingError::OutsideCrop);
        }
        let (crop_x, crop_y) = (crop_x.clamp(0.0, 1.0), crop_y.clamp(0.0, 1.0));
        // Обратное к таблице из документации модуля: сенсор `(sx, sy)` → прямой `(u, v)`.
        let (u, v) = match self.quarter_turns {
            1 => (1.0 - crop_y, crop_x),
            2 => (1.0 - crop_x, 1.0 - crop_y),
            3 => (crop_y, 1.0 - crop_x),
            _ => (crop_x, crop_y),
        };
        let u = if self.mirror == MirrorMode::Horizontal {
            1.0 - u
        } else {
            u
        };
        Ok(Point::new(
            self.frame.min_x() + u * self.frame.size.width,
            self.frame.min_y() + v * self.frame.size.height,
        ))
    }
}
