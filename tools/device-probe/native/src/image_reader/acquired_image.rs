//! Захваченный `AImage` с acquire-fence и явным освобождением.
//!
//! Главный инвариант проекта: `AImage` нельзя отдавать обратно камере, пока GPU
//! не закончил читать его буфер. Поэтому освобождение только через
//! [`AcquiredImage::release_after_gpu`] (с release-fence) либо
//! [`AcquiredImage::release_now`] (вызывать строго после `vkQueueWaitIdle`).
//! Если ни то ни другое не вызвано (ранний выход/паника) — `Drop` делает безопасный
//! `AImage_delete` (на этом пути GPU-работа уже не может быть в полёте: см. import_test).

use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::ptr::{self, NonNull};

use ndk_sys::{
    AHardwareBuffer, AHardwareBuffer_Desc, AHardwareBuffer_describe, AImage, AImage_delete, AImage_deleteAsync,
    AImage_getHardwareBuffer, AImage_getPlaneRowStride, AImageReader, AImageReader_acquireNextImageAsync,
};

use crate::status_names::media_status_name;

/// Результат попытки захватить кадр.
pub enum AcquireOutcome {
    /// Кадр получен.
    Image(AcquiredImage),
    /// Кадров пока нет (`AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE`) — штатно, нужно подождать.
    NoBufferYet,
    /// Любая другая ошибка NDK.
    Failed(String),
}

/// `AImage*` + acquire-fence. Освобождается ровно один раз.
pub struct AcquiredImage {
    raw: Option<NonNull<AImage>>,
    acquire_fence: Option<OwnedFd>,
}

// Коды media_status_t, которые обрабатываем особо.
const STATUS_OK: i32 = 0;
const STATUS_NO_BUFFER_AVAILABLE: i32 = -30001;

impl AcquiredImage {
    /// `AImageReader_acquireNextImageAsync`.
    ///
    /// # Safety
    /// `reader` обязан быть живым `AImageReader*` на всё время вызова.
    pub unsafe fn acquire_next(reader: *mut AImageReader) -> AcquireOutcome {
        let mut image: *mut AImage = ptr::null_mut();
        let mut fence_fd: i32 = -1;
        // SAFETY: reader жив по контракту; оба out-параметра валидны.
        let status = unsafe { AImageReader_acquireNextImageAsync(reader, &mut image, &mut fence_fd) };
        if status.0 == STATUS_NO_BUFFER_AVAILABLE {
            return AcquireOutcome::NoBufferYet;
        }
        if status.0 != STATUS_OK {
            return AcquireOutcome::Failed(media_status_name(status.0));
        }
        let Some(raw) = NonNull::new(image) else {
            return AcquireOutcome::Failed("acquireNextImageAsync: OK, но image == null".to_owned());
        };
        // SAFETY: при fence_fd >= 0 NDK передаёт нам владение дескриптором.
        let acquire_fence = (fence_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fence_fd) });
        AcquireOutcome::Image(Self {
            raw: Some(raw),
            acquire_fence,
        })
    }

    /// Забирает acquire-fence (владение переходит вызывающему). `None` — кадр уже готов.
    pub fn take_acquire_fence(&mut self) -> Option<OwnedFd> {
        self.acquire_fence.take()
    }

    fn image_ptr(&self) -> Option<*mut AImage> {
        self.raw.map(NonNull::as_ptr)
    }

    /// `AHardwareBuffer*` кадра (без передачи владения; живёт, пока жив `AImage`).
    pub fn hardware_buffer(&self) -> Result<*mut AHardwareBuffer, String> {
        let image = self.image_ptr().ok_or("AImage уже освобождён")?;
        let mut buffer: *mut AHardwareBuffer = ptr::null_mut();
        // SAFETY: image жив (raw = Some); buffer — валидный out.
        let status = unsafe { AImage_getHardwareBuffer(image, &mut buffer) };
        if status.0 != STATUS_OK {
            return Err(format!("AImage_getHardwareBuffer: {}", media_status_name(status.0)));
        }
        if buffer.is_null() {
            return Err("AImage_getHardwareBuffer: OK, но buffer == null".to_owned());
        }
        Ok(buffer)
    }

    /// Row stride плоскости 0 в байтах; `None`, если NDK не определяет (напр. RAW_PRIVATE).
    pub fn plane0_row_stride_bytes(&self) -> Option<i32> {
        let image = self.image_ptr()?;
        let mut stride: i32 = 0;
        // SAFETY: image жив; stride — валидный out.
        let status = unsafe { AImage_getPlaneRowStride(image, 0, &mut stride) };
        (status.0 == STATUS_OK && stride > 0).then_some(stride)
    }

    /// Освобождение с release-fence: GPU-чтение, на которое указывает fence, может ещё идти.
    /// `None` = GPU ничего не читал (fence не нужен, передаём -1).
    /// Владение `release_fence` переходит NDK.
    pub fn release_after_gpu(mut self, release_fence: Option<OwnedFd>) {
        if let Some(image) = self.raw.take() {
            let fence_fd = release_fence.map_or(-1, IntoRawFd::into_raw_fd);
            // SAFETY: image жив и освобождается один раз (raw взят через take);
            // fd уходит во владение NDK.
            unsafe { AImage_deleteAsync(image.as_ptr(), fence_fd) };
        }
    }

    /// Синхронное освобождение. Вызывать только когда GPU гарантированно не использует буфер
    /// (после `vkQueueWaitIdle`) либо GPU его не трогал.
    pub fn release_now(mut self) {
        self.delete_sync();
    }

    fn delete_sync(&mut self) {
        if let Some(image) = self.raw.take() {
            // SAFETY: image жив и удаляется один раз.
            unsafe { AImage_delete(image.as_ptr()) };
        }
    }
}

impl Drop for AcquiredImage {
    fn drop(&mut self) {
        self.delete_sync();
    }
}

/// `AHardwareBuffer_describe` в безопасной обёртке.
///
/// # Safety
/// `buffer` — валидный `AHardwareBuffer*`.
pub unsafe fn describe_hardware_buffer(buffer: *mut AHardwareBuffer) -> AHardwareBuffer_Desc {
    // SAFETY: Desc — POD, нулевая инициализация допустима.
    let mut desc: AHardwareBuffer_Desc = unsafe { std::mem::zeroed() };
    // SAFETY: buffer валиден по контракту, desc — валидный out.
    unsafe { AHardwareBuffer_describe(buffer, &mut desc) };
    desc
}
