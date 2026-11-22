//! Захваченный кадр снимка и проявленные данные.
//!
//! [`StillFrameFacts`] — всё, что нужно файлам о снятом кадре: паспорт потока (уровни, мозаика,
//! DNG-матрицы), метаданные результата (выдержка, ISO), наша нейтраль, кадрирование, поворот.
//! Пиксели идут отдельно: сырые 16 бит ([`RawSamples16`], только когда нужен DNG) и картинка RGBA8
//! ([`Rgba8Image`], только для JPEG — HEIF рендер рисует прямо в кодер).

use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::{FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize};
use serde::Serialize;

use crate::frame_choice::FrameChoice;
use crate::plan::{StillId, StillOutputs};

/// Поворот по часовой стрелке, который нужно применить к картинке в ориентации сенсора, чтобы она
/// стала «как держали телефон». Пиксели не вращаются: поворот пишется тегом (DNG/Exif `Orientation`,
/// `HeifWriter` `setRotation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StillRotation {
    None,
    Clockwise90,
    Clockwise180,
    Clockwise270,
}

impl StillRotation {
    /// Угол в градусах по часовой стрелке.
    pub fn degrees(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 90,
            Self::Clockwise180 => 180,
            Self::Clockwise270 => 270,
        }
    }

    /// Поворот на угол, кратный 90° (любой знак и величина); иначе `None`.
    pub fn from_degrees(degrees: i32) -> Option<Self> {
        if degrees % 90 != 0 {
            return None;
        }
        match degrees.rem_euclid(360) {
            0 => Some(Self::None),
            90 => Some(Self::Clockwise90),
            180 => Some(Self::Clockwise180),
            _ => Some(Self::Clockwise270),
        }
    }
}

/// Нейтраль камеры для DNG `AsShotNeutral` (DNG 1.7, раздел 6, тег 50728): цвет белого в
/// пространстве сенсора, нормированный так, что максимум = 1. Это НАША нейтраль (наш AWB того кадра),
/// а не подсказка HAL (решение 9).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AsShotNeutral(pub [f64; 3]);

/// Цифровая подтяжка экспозиции, с которой видоискатель показывал этот кадр (`k` решения 72), в EV.
/// Снимок повторяет её (решение 72), а DNG передаёт как `BaselineExposure`, чтобы редактор показал
/// ту же яркость.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DigitalGainEv(pub f32);

/// Что известно о снятом кадре. Создаёт рендер в момент захвата («кадр захвачен»).
#[derive(Debug, Clone, PartialEq)]
pub struct StillFrameFacts {
    pub id: StillId,
    pub outputs: StillOutputs,
    /// Паспорт потока снятого кадра: камера, уровни, мозаика, полезная область, DNG-калибровка.
    pub stream: Arc<StreamPassport>,
    pub sensor_timestamp: SensorTimestamp,
    /// Метаданные результата этого кадра. Кадр без метаданных снимок не берёт (решение 42),
    /// поэтому здесь они есть всегда.
    pub capture: FrameCaptureDetails,
    pub choice: FrameChoice,
    /// Область снимка в координатах буфера кадра: кадрирование по соотношению и зуму внутри
    /// полезной области (та же формула, что у видоискателя).
    pub crop: PixelRect,
    /// Поворот к «как держали телефон».
    pub rotation: StillRotation,
    pub neutral: AsShotNeutral,
    pub digital_gain: DigitalGainEv,
}

impl StillFrameFacts {
    /// Размер основной картинки (в ориентации сенсора, до поворота тегом).
    pub fn primary_size(&self) -> PixelSize {
        self.crop.size
    }
}

/// Хранилище пикселей проявленного снимка.
///
/// Байты живут там, куда их положил GPU (память, видимая процессору), и не копируются по дороге:
/// DNG пишет их в файл как есть (порядок байт TIFF `II`, little-endian), JPEG отдаёт платформе.
/// Реализация держит GPU-буфер живым, пока жив последний `Arc`.
pub trait PixelStorage: Send + Sync {
    /// Все байты хранилища.
    fn bytes(&self) -> &[u8];
}

/// Хранилище в обычной памяти процесса (тесты, CPU-пути).
#[derive(Clone, PartialEq, Eq)]
pub struct OwnedPixels(pub Vec<u8>);

impl PixelStorage for OwnedPixels {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Ошибка сборки пиксельного буфера: размер хранилища не совпал с размером картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("в хранилище {actual} байт, а картинке {size:?} нужно {expected}")]
pub struct PixelStorageSizeMismatch {
    pub size: PixelSize,
    pub expected: u64,
    pub actual: u64,
}

fn check_storage(
    size: PixelSize,
    bytes_per_pixel: u64,
    storage: &dyn PixelStorage,
) -> Result<(), PixelStorageSizeMismatch> {
    let expected = size.pixel_count().saturating_mul(bytes_per_pixel);
    let actual = u64::try_from(storage.bytes().len()).unwrap_or(u64::MAX);
    if expected == actual {
        Ok(())
    } else {
        Err(PixelStorageSizeMismatch { size, expected, actual })
    }
}

/// Сырые отсчёты кадра, распакованные в 16 бит (RAW10 → u16 без масштабирования), строками,
/// little-endian. Покрывают ВСЮ полезную область сенсора: DNG хранит полный кадр, а кадрирование
/// пишет тегами `DefaultCropOrigin/Size` — редактор может вернуть края.
#[derive(Clone)]
pub struct RawSamples16 {
    size: PixelSize,
    storage: Arc<dyn PixelStorage>,
}

impl RawSamples16 {
    /// Собирает отсчёты; хранилище обязано держать ровно `width * height * 2` байт.
    pub fn new(size: PixelSize, storage: Arc<dyn PixelStorage>) -> Result<Self, PixelStorageSizeMismatch> {
        check_storage(size, 2, storage.as_ref())?;
        Ok(Self { size, storage })
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Отсчёты как байты little-endian (2 байта на отсчёт, строка за строкой).
    pub fn le_bytes(&self) -> &[u8] {
        self.storage.bytes()
    }

    /// Отсчёт в точке (для тестов и проверок); вне кадра — `None`.
    pub fn sample(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        let index = (u64::from(y) * u64::from(self.size.width) + u64::from(x)) * 2;
        let index = usize::try_from(index).ok()?;
        let bytes = self.storage.bytes().get(index..index + 2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

impl std::fmt::Debug for RawSamples16 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawSamples16")
            .field("size", &self.size)
            .field("bytes", &self.storage.bytes().len())
            .finish()
    }
}

/// Готовая картинка в sRGB 8 бит, RGBA построчно (альфа = 255). Только для JPEG.
#[derive(Clone)]
pub struct Rgba8Image {
    size: PixelSize,
    storage: Arc<dyn PixelStorage>,
}

impl Rgba8Image {
    /// Собирает картинку; хранилище обязано держать ровно `width * height * 4` байт.
    pub fn new(size: PixelSize, storage: Arc<dyn PixelStorage>) -> Result<Self, PixelStorageSizeMismatch> {
        check_storage(size, 4, storage.as_ref())?;
        Ok(Self { size, storage })
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Байты RGBA построчно.
    pub fn rgba_bytes(&self) -> &[u8] {
        self.storage.bytes()
    }
}

impl std::fmt::Debug for Rgba8Image {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Rgba8Image")
            .field("size", &self.size)
            .field("bytes", &self.storage.bytes().len())
            .finish()
    }
}
