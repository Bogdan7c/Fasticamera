//! Типы запроса проявления и его результата.

use std::sync::Arc;

use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::rgba8_image::Rgba8StorageImage;
use isp_model::params::IspParams;
use still_contract::shot::Rgba8Image;

use super::capture::RawCapture;

/// Нужна ли копия готовой картинки в память процессора (для JPEG).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbaReadback {
    /// Не нужна: картинку заберёт кодер HEIF прямо с GPU (копией в surface). Памяти хоста не тратится.
    NotNeeded,
    /// Нужна: после проявления доступен [`DevelopedPhoto::host_rgba8`] (байты RGBA в памяти, видимой CPU).
    ForJpeg,
}

/// Всё, что нужно, чтобы начать проявление снимка.
#[derive(Debug, Clone, Copy)]
pub struct DevelopRequest<'a> {
    /// Сырые отсчёты снимка (результат захвата).
    pub raw: &'a RawCapture,
    /// Параметры обработки (`DemosaicMethod::Quality`; их собирает рендер из паспорта и результата кадра).
    pub params: &'a IspParams,
    /// Область снимка в КООРДИНАТАХ БУФЕРА КАДРА (как `StillFrameFacts::crop`): целиком внутри полезной области.
    pub crop: PixelRect,
    /// Нужна ли копия в память процессора.
    pub readback: RgbaReadback,
}

/// Что известно сразу после начала проявления.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevelopmentStart {
    /// Сколько полос придётся записать (столько раз звать `record_next_slice` до `Finished`).
    pub slice_count: usize,
    /// Размер картинки снимка.
    pub image_size: PixelSize,
    /// Область проявления (crop + ореол) по ширине и наибольшая высота области полосы, пиксели: из них
    /// складывается память плоскостей.
    pub widest_region: PixelSize,
}

/// Итог записи следующей полосы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceOutcome {
    /// Полоса записана в командный буфер; осталось записать ещё `remaining` полос (0 — эта была последней).
    Recorded {
        /// Сколько полос ещё не записано.
        remaining: usize,
    },
    /// Записывать нечего: все полосы уже записаны (в командный буфер ничего не добавлено).
    Finished,
}

/// Готовый снимок.
pub struct DevelopedPhoto {
    pub(crate) image: Arc<Rgba8StorageImage>,
    pub(crate) size: PixelSize,
    pub(crate) host_rgba8: Option<Rgba8Image>,
}

impl std::fmt::Debug for DevelopedPhoto {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "DevelopedPhoto({}x{}, host_rgba8: {})",
            self.size.width,
            self.size.height,
            self.host_rgba8.is_some()
        )
    }
}

impl DevelopedPhoto {
    /// Размер картинки.
    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Картинка RGBA8 на GPU — для копирования в surface кодера HEIF. Раскладка —
    /// [`Rgba8StorageImage::TRANSFER_SOURCE_LAYOUT`]: копировать можно сразу, барьеры не нужны; стадия
    /// чтения — `COPY`, доступ — `TRANSFER_READ`. Картинка должна жить до конца копирования (клон `Arc`).
    pub fn gpu_image(&self) -> &Arc<Rgba8StorageImage> {
        &self.image
    }

    /// Картинка в памяти процессора (JPEG): есть, только если запрошен [`RgbaReadback::ForJpeg`].
    pub fn host_rgba8(&self) -> Option<&Rgba8Image> {
        self.host_rgba8.as_ref()
    }

    /// Забирает картинку для JPEG (оставляя `None`).
    pub fn take_host_rgba8(&mut self) -> Option<Rgba8Image> {
        self.host_rgba8.take()
    }
}
