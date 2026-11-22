//! Мост «буфер GPU → `still_contract::shot::PixelStorage`»: потребители снимка (DNG, JPEG) получают байты
//! прямо над отображённой памятью, без копирования.

use gpu_vulkan::host_readback::{HostReadbackBuffer, SealedHostBytes};
use gpu_vulkan::one_shot_queue::CompletedWork;
use still_contract::shot::PixelStorage;

use super::error::PhotoError;

/// Запечатанный буфер GPU как хранилище пикселей. Держит буфер (память, отображение) живым, пока жив
/// последний `Arc` на него.
///
/// # Потоки и уничтожение
/// `PixelStorage` требует `Send + Sync`: `SealedHostBytes` их обеспечивает (после запечатывания CPU только
/// читает, GPU только читает; запись невозможна типами). Последний владелец уничтожает буфер на СВОЁМ потоке
/// (например, на потоке записи файла): `vkUnmapMemory`/`vkDestroyBuffer`/`vkFreeMemory` требуют внешней
/// синхронизации лишь по самому уничтожаемому объекту, а он принадлежит одному владельцу; GPU к этому
/// моменту закончил с буфером, потому что сабмиты, читавшие его (проявление), держат свой клон `Arc` до
/// подтверждения fence (`PhotoDeveloper::slice_completed`).
pub(crate) struct GpuPixels(SealedHostBytes);

impl GpuPixels {
    pub(crate) fn new(bytes: SealedHostBytes) -> Self {
        Self(bytes)
    }

    pub(crate) fn sealed(&self) -> &SealedHostBytes {
        &self.0
    }
}

impl PixelStorage for GpuPixels {
    fn bytes(&self) -> &[u8] {
        self.0.bytes()
    }
}

impl std::fmt::Debug for GpuPixels {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "GpuPixels({:?})", self.0)
    }
}

/// Запечатывает буфер доказательством; отказ (чужое доказательство или нет отметки записи) оставляет буфер в
/// утечке: GPU мог ещё писать в него, а уничтожать занятую память нельзя (соглашение крейта).
pub(crate) fn seal_or_leak(buffer: HostReadbackBuffer, proof: &CompletedWork) -> Result<SealedHostBytes, PhotoError> {
    buffer.seal(proof).map_err(|refusal| {
        let what = refusal.to_string();
        tracing::error!(target: "fasticamera::isp", %what, "буфер не запечатан: оставлен в утечку");
        std::mem::forget(refusal.buffer);
        PhotoError::SealRefused { what }
    })
}
