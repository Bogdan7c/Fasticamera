//! Ошибки проявления снимка: причины не сливаются в одну — вызывающему (рендер, пакет P4) важно, сломан ли GPU,
//! неверен ли конфиг, не подходят ли кадр или параметры, либо он нарушил порядок вызовов.

use gpu_vulkan::error::GpuError;

use crate::frame_style::FrameStyleError;
use crate::lens_shading_map::{LensShadingMapBufferError, LensShadingMapError};
use crate::look::LookLutError;
use crate::look::lut_buffer::LookLutBufferError;
use crate::quality::QualityError;

/// Ошибка проявления снимка.
#[derive(Debug, thiserror::Error)]
pub enum PhotoError {
    /// Ошибка уровня Vulkan-набора (код Vulkan, таймаут, неверный аргумент ресурса).
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
    /// Конфиг противоречив (проверяется до создания объектов Vulkan).
    #[error("неверный конфиг проявления снимка: {what}")]
    InvalidConfig {
        /// Что именно не так.
        what: String,
    },
    /// Вход не подходит: полезная область, источник кадра или область снимка (crop).
    #[error("вход проявления снимка не подходит: {what}")]
    InvalidInput {
        /// Что именно не так.
        what: String,
    },
    /// Параметры обработки требуют того, чего GPU-конвейер не умеет (честный отказ вместо догадки).
    #[error("параметры обработки не поддержаны проявлением снимка: {what}")]
    UnsupportedParams {
        /// Что именно не поддержано.
        what: String,
    },
    /// Карта коррекции виньетирования не помещается в буфер или область слишком мала: коррекция не
    /// применяется молча, проявка отвергается.
    #[error("карта виньетирования: {0}")]
    LensShadingMap(#[from] LensShadingMapError),
    /// Профиль выхода (Log, HDR, 10 бит) ещё не считается: кадр отвергается, а не рисуется другим профилем.
    #[error("{0}")]
    UnsupportedOutputProfile(#[from] isp_model::output::UnsupportedOutputProfile),
    /// Таблица вида (LUT) не помещается в буфер: вид не применяется молча, кадр отвергается.
    #[error("таблица вида: {0}")]
    LookLut(#[from] LookLutError),
    /// Предыдущая операция того же владельца ещё не завершена: два захвата или две проявки сразу делать нельзя.
    #[error("занято: {what}")]
    Busy {
        /// Что именно не завершено.
        what: &'static str,
    },
    /// Буфер чтения хостом нельзя запечатать: доказательство завершения не той работы, что в него писала, либо
    /// вызывающий не сообщил буферу, какая работа пишет (`note_*_submitted`). Буфер оставлен в утечку: GPU мог
    /// ещё писать в него (соглашение крейта: лучше утечка, чем уничтожение занятого).
    #[error("запечатать буфер нельзя: {what}")]
    SealRefused {
        /// Причина отказа.
        what: String,
    },
    /// Операция вызвана не в свой черёд (нет активной проявки, полоса записана, но fence не подтверждён, …).
    #[error("нарушен порядок вызовов проявления: {what}")]
    OutOfOrder {
        /// Что именно нарушено.
        what: &'static str,
    },
}

impl From<QualityError> for PhotoError {
    /// Стенд V4 и снимок делят проверки кадра и параметров; их ошибки переводятся вариант в вариант.
    fn from(error: QualityError) -> Self {
        match error {
            QualityError::Gpu(gpu) => Self::Gpu(gpu),
            QualityError::InvalidConfig { what } => Self::InvalidConfig { what },
            QualityError::InvalidInput { what } => Self::InvalidInput { what },
            QualityError::UnsupportedParams { what } => Self::UnsupportedParams { what },
            QualityError::LensShadingMap(map) => Self::LensShadingMap(map),
            QualityError::LookLut(lut) => Self::LookLut(lut),
            QualityError::UnsupportedOutputProfile(profile) => Self::UnsupportedOutputProfile(profile),
        }
    }
}

impl From<LensShadingMapBufferError> for PhotoError {
    fn from(error: LensShadingMapBufferError) -> Self {
        match error {
            LensShadingMapBufferError::Map(map) => Self::LensShadingMap(map),
            LensShadingMapBufferError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl From<FrameStyleError> for PhotoError {
    fn from(error: FrameStyleError) -> Self {
        match error {
            FrameStyleError::Look(look) => Self::from(look),
            FrameStyleError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

impl From<LookLutBufferError> for PhotoError {
    fn from(error: LookLutBufferError) -> Self {
        match error {
            LookLutBufferError::Lut(lut) => Self::LookLut(lut),
            LookLutBufferError::Gpu(gpu) => Self::Gpu(gpu),
        }
    }
}

#[cfg(test)]
mod tests {
    use isp_model::output::{OutputRendering, UnsupportedOutputProfile};

    use super::*;

    /// Отказ по профилю выхода от общей проверки качественного пути доходит до снимка своим вариантом, а не
    /// общим `UnsupportedParams`.
    #[test]
    fn unsupported_output_profile_keeps_its_variant_on_the_way_to_photo_error() {
        let refusal = UnsupportedOutputProfile::Rendering(OutputRendering::Hdr);
        let photo = PhotoError::from(QualityError::UnsupportedOutputProfile(refusal));
        assert!(matches!(photo, PhotoError::UnsupportedOutputProfile(error) if error == refusal));
    }
}
