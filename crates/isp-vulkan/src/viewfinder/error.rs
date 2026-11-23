//! Ошибки ISP видоискателя. Все проверки — до записи команд: при ошибке командный буфер не тронут.

use camera_contract::geometry::PixelRect;
use camera_contract::viewfinder_crop::ViewfinderCropError;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_model::image::DisplayGamut;
use isp_model::params::IspParamsVersion;
use isp_model::statistics::{InvalidAwbStatistics, TileCountMismatch};

use crate::frame_style::FrameStyleError;
use crate::lens_shading_map::{LensShadingMapBufferError, LensShadingMapError};
use crate::look::LookLutError;
use crate::look::lut_buffer::LookLutBufferError;

/// Почему кадр нельзя обработать (или объект нельзя создать).
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum ViewfinderError {
    /// Параметры собраны другой моделью обработки (сравнили бы разное).
    #[error("версия IspParams {actual:?}, а шейдер написан под {expected:?}")]
    ParamsVersionMismatch {
        /// Версия, под которую написан шейдер.
        expected: IspParamsVersion,
        /// Версия параметров кадра.
        actual: IspParamsVersion,
    },
    /// Параметры просят то, чего слитый проход видоискателя не делает.
    #[error("параметры не для видоискателя: {what}")]
    UnsupportedParams {
        /// Что именно (демозаик Quality, коррекция виньетирования картой).
        what: &'static str,
    },
    /// Матрица экрана в параметрах посчитана под другую гамму, чем у цели показа.
    #[error("параметры посчитаны под гамму {params:?}, а экран {display:?}")]
    GamutMismatch {
        /// Гамма, под которую собраны параметры.
        params: DisplayGamut,
        /// Цветовое пространство цели показа.
        display: DisplayColorSpace,
    },
    /// Геометрия кадра противоречива (полезная область, кадрирование, буфер).
    #[error("геометрия кадра: {what}")]
    InvalidGeometry {
        /// Что не так.
        what: String,
    },
    /// Кадрирование не разрешилось.
    #[error("кадрирование: {0}")]
    Crop(#[from] ViewfinderCropError),
    /// Настройки статистики AWB не подходят к кадрированию.
    #[error("статистика AWB: {0}")]
    AwbStatistics(#[from] InvalidAwbStatistics),
    /// Буфер статистики прочитан не того размера (внутренняя ошибка).
    #[error("статистика AWB: {0}")]
    AwbTileCount(#[from] TileCountMismatch),
    /// Карта коррекции виньетирования не помещается в буфер или область слишком мала: коррекция не
    /// применяется молча, кадр отвергается.
    #[error("карта виньетирования: {0}")]
    LensShadingMap(#[from] LensShadingMapError),
    /// Профиль выхода (Log, HDR, 10 бит) ещё не считается: кадр отвергается, а не рисуется другим профилем.
    #[error("{0}")]
    UnsupportedOutputProfile(#[from] isp_model::output::UnsupportedOutputProfile),
    /// Таблица вида (LUT) не помещается в буфер: вид не применяется молча, кадр отвергается.
    #[error("таблица вида: {0}")]
    LookLut(#[from] LookLutError),
    /// Конфигурация не имеет смысла.
    #[error("конфигурация видоискателя: {what}")]
    InvalidConfig {
        /// Что не так.
        what: String,
    },
    /// Слот кадра вне кольца.
    #[error("слот кадра {slot} вне 0..{slots}")]
    FrameSlotOutOfRange {
        /// Запрошенный слот.
        slot: usize,
        /// Число слотов.
        slots: usize,
    },
    /// Ошибка GPU-набора.
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
}

impl From<LensShadingMapBufferError> for ViewfinderError {
    fn from(error: LensShadingMapBufferError) -> Self {
        match error {
            LensShadingMapBufferError::Map(map) => Self::LensShadingMap(map),
            LensShadingMapBufferError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl From<FrameStyleError> for ViewfinderError {
    fn from(error: FrameStyleError) -> Self {
        match error {
            FrameStyleError::Look(look) => Self::from(look),
            FrameStyleError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl From<LookLutBufferError> for ViewfinderError {
    fn from(error: LookLutBufferError) -> Self {
        match error {
            LookLutBufferError::Lut(lut) => Self::LookLut(lut),
            LookLutBufferError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl ViewfinderError {
    /// Ошибка геометрии с текстом про прямоугольник.
    pub(crate) fn geometry(what: impl Into<String>) -> Self {
        Self::InvalidGeometry { what: what.into() }
    }

    /// Ошибка «прямоугольник не выровнен по ячейке 2×2».
    pub(crate) fn not_cell_aligned(name: &str, rect: PixelRect) -> Self {
        Self::geometry(format!(
            "{name} {rect:?}: края и размеры должны быть чётными (ячейка CFA 2×2), размер ≥ 2×2"
        ))
    }
}
