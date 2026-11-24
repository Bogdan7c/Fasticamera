//! Источник RAW-кадра для адаптеров: кто владеет памятью кадра.
//!
//! `MosaicSource` — набор сырых хэндлов Vulkan без владельца. Создать буфер под него без «сырого» `ash::Device`
//! `isp-vulkan` не может (решение владельца в 07), поэтому память кадра принадлежит ВЫЗЫВАЮЩЕМУ:
//! он передаёт [`RawFrameHolder`]. Держателем служит буфер `HostRawFrame` из `gpu-vulkan`
//! ([`synthetic_raw10_holder`], без `test-support`); сценарий на телефоне даст свой держатель.

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::saturation::{HostRawFrame, RawFrameError};

use super::error::SaturationWorkError;
use super::scene::Raw10Frame;

/// Владелец памяти RAW-кадра. Пока он жив, [`Self::source`] пригоден для чтения compute-шейдерами.
pub trait RawFrameHolder {
    /// Источник для ISP (чтение). Один и тот же кадр на каждый вызов.
    fn source(&self) -> MosaicSource;
}

/// Держатель на обычной памяти Vulkan (`HostRawFrame` из `gpu-vulkan`): работает и в release, без `test-support`.
impl RawFrameHolder for HostRawFrame {
    fn source(&self) -> MosaicSource {
        HostRawFrame::source(self)
    }
}

/// Загружает готовый RAW10-кадр в буфер на обычной памяти Vulkan (владение «наше», барьеров камеры нет).
pub fn synthetic_raw10_holder(
    context: &GpuContext,
    frame: &Raw10Frame,
) -> Result<Box<dyn RawFrameHolder>, SaturationWorkError> {
    let holder = HostRawFrame::new(context, frame.width, frame.height, frame.row_stride_bytes, &frame.bytes).map_err(
        |error| match error {
            RawFrameError::Gpu(gpu) => SaturationWorkError::Gpu(gpu),
            other => SaturationWorkError::invalid_config(other.to_string()),
        },
    )?;
    Ok(Box::new(holder))
}
