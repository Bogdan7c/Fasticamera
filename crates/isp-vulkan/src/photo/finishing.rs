//! Шаг отделки проявки (14g): миниатюра и копии готовой картинки — ОТДЕЛЬНЫЙ шаг после последней полосы вывода.
//!
//! Зачем отдельно: рендер записывает по одной полосе на кадр видоискателя (`slices_per_frame = 1`). Уменьшение
//! 12 Мп на телефоне, будучи в последней полосе, удваивало пропуски кадров видоискателя (8 → 15 за прогон,
//! доля выбросов серии пересекала порог 0,15). Отдельный шаг уходит в свой кадр: бюджет полос растягивает работу.
//!
//! Шаг: зависимость compute → compute (читаем то, что писал выходной проход) → миниатюра (если есть) → переход
//! основной картинки к копированию → копия в память хоста (JPEG). Миниатюры нет — шаг всё равно делает переход
//! и копию. Доказательство завершения ЭТОГО шага запечатывает оба буфера хоста (`last_proof`).

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::record_compute_pass_dependency;
use gpu_vulkan::host_readback::HostReadbackBuffer;
use gpu_vulkan::rgba8_image::Rgba8StorageImage;

use super::error::PhotoError;
use super::thumbnail_pass::{ThumbnailJob, ThumbnailPass};
use super::timing::{PhotoGpuPoint, PhotoGpuTimer};

/// Всё, что шаг отделки читает и пишет (владеет ими проявка).
pub(crate) struct FinishingParts<'a> {
    /// Проявленная картинка снимка (после выходного прохода в раскладке GENERAL).
    pub image: &'a Rgba8StorageImage,
    /// Буфер копии для JPEG (`None` — HEIF: картинку заберёт кодер с GPU).
    pub readback: Option<&'a HostReadbackBuffer>,
    pub thumbnail: &'a ThumbnailJob,
    /// Пайплайн миниатюры; `None` — миниатюры не будет (у задания `thumbnail` тогда `Skipped`).
    pub thumbnail_pass: Option<&'a ThumbnailPass>,
    /// Метки времени (`None` — прибор выключен).
    pub timer: &'a mut Option<PhotoGpuTimer>,
}

/// Записывает шаг отделки в `command_buffer`.
///
/// Первая зависимость охватывает все прежние команды очереди, в том числе прошлых сабмитов (область барьера —
/// порядок отправки), поэтому полосы вывода, записанные в ДРУГИХ сабмитах, к этому моменту записали картинку.
/// Ошибка записи миниатюры срывает шаг (часть её команд уже в буфере); на деле путь недостижим: размеры и привязки
/// проверены в `ThumbnailJob::begin`.
pub(crate) fn record_finishing_step(
    context: &GpuContext,
    command_buffer: vk::CommandBuffer,
    parts: FinishingParts<'_>,
) -> Result<(), PhotoError> {
    record_compute_pass_dependency(context, command_buffer);
    if let Some(pass) = parts.thumbnail_pass {
        parts.thumbnail.record(pass, command_buffer)?;
    }
    PhotoGpuTimer::mark_if_enabled(parts.timer, command_buffer, PhotoGpuPoint::Thumbnail)?;
    parts.image.record_compute_write_to_transfer_read(command_buffer);
    if let Some(readback) = parts.readback {
        parts.image.record_copy_to_host_readback(command_buffer, readback)?;
    }
    PhotoGpuTimer::mark_if_enabled(parts.timer, command_buffer, PhotoGpuPoint::Finishing)?;
    Ok(())
}
