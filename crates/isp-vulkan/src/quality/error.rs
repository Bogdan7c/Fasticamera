//! Ошибки качественного конвейера: причины не сливаются в одну — вызывающему (render-loop, стенд V4)
//! важно, сломан ли GPU, неверен ли конфиг, не подходит ли кадр или параметры обработки.

use gpu_vulkan::error::GpuError;

use crate::frame_style::FrameStyleError;
use crate::lens_shading_map::{LensShadingMapBufferError, LensShadingMapError};
use crate::look::LookLutError;
use crate::look::lut_buffer::LookLutBufferError;

/// Ошибка качественного конвейера полного кадра.
#[derive(Debug, thiserror::Error)]
pub enum QualityError {
    /// Ошибка уровня Vulkan-набора (код Vulkan, таймаут, неверный аргумент ресурса).
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
    /// Конфиг противоречив (проверяется до создания объектов Vulkan).
    #[error("неверный конфиг качественного конвейера: {what}")]
    InvalidConfig {
        /// Что именно не так.
        what: String,
    },
    /// Кадр не подходит к конвейеру: полезная область, источник, слот, приёмник.
    #[error("кадр не подходит к качественному конвейеру: {what}")]
    InvalidInput {
        /// Что именно не так.
        what: String,
    },
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
    /// Параметры обработки требуют того, чего GPU-конвейер не умеет (честный отказ вместо догадки).
    #[error("параметры обработки не поддержаны качественным конвейером: {what}")]
    UnsupportedParams {
        /// Что именно не поддержано.
        what: String,
    },
}

impl From<LensShadingMapBufferError> for QualityError {
    fn from(error: LensShadingMapBufferError) -> Self {
        match error {
            LensShadingMapBufferError::Map(map) => Self::LensShadingMap(map),
            LensShadingMapBufferError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl From<FrameStyleError> for QualityError {
    fn from(error: FrameStyleError) -> Self {
        match error {
            FrameStyleError::Look(look) => Self::from(look),
            FrameStyleError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl From<LookLutBufferError> for QualityError {
    fn from(error: LookLutBufferError) -> Self {
        match error {
            LookLutBufferError::Lut(lut) => Self::LookLut(lut),
            LookLutBufferError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}
