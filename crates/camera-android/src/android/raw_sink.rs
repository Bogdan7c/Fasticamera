//! Временный RAW-приёмник: `AImageReader`, в который камера пишет кадры, пока нет настоящего
//! конвейера.
//!
//! ВРЕМЕННО до сессии 05: заменяется лизингом `RawFrameLease`. Зачем он нужен уже сейчас: HAL
//! выбирает режим сенсора по RAW-потоку, поэтому подтвердить 60 к/с YUV-заглушкой нельзя (решение
//! главного агента). Кадры отпускаются сразу слушателем, пиксели не читаются.
//!
//! usage — `GPU_SAMPLED_IMAGE` (факт 00 C4): `GPU_DATA_BUFFER` для RAW gralloc не выделяет и камера
//! молча отдаёт 0 кадров.

use std::ptr::{self, NonNull};
use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;
use camera_core::backend::BackendFailure;
use camera_ndk_sys as ndk;

use crate::diagnostics::BackendDiagnostics;
use crate::status_mapping::media_backend_failure;
use crate::tags;

use super::reader_callback::{ReaderContext, reader_on_image_available};

/// Владелец `AImageReader` с окном и контекстом слушателя.
pub(super) struct TemporaryRawSink {
    reader: NonNull<ndk::AImageReader>,
    window: NonNull<ndk::ANativeWindow>,
    context: Arc<ReaderContext>,
}

impl TemporaryRawSink {
    pub(super) fn create(
        size: PixelSize,
        format: RawFormat,
        max_images: u32,
        diagnostics: Arc<BackendDiagnostics>,
    ) -> Result<Self, BackendFailure> {
        let invalid = |what: &str| BackendFailure {
            operation: "AImageReader_newWithUsage",
            code: 0,
            detail: format!("{what} не помещается в int32"),
        };
        let width = i32::try_from(size.width).map_err(|_| invalid("ширина"))?;
        let height = i32::try_from(size.height).map_err(|_| invalid("высота"))?;
        let max_images = i32::try_from(max_images).map_err(|_| invalid("maxImages"))?;
        let format_code = match format {
            RawFormat::Raw10 => tags::IMAGE_FORMAT_RAW10,
            RawFormat::Raw16 => tags::IMAGE_FORMAT_RAW16,
        };

        let mut reader: *mut ndk::AImageReader = ptr::null_mut();
        // SAFETY: аргументы — обычные числа, `reader` — валидное выходное место.
        let status = unsafe {
            ndk::AImageReader_newWithUsage(
                width,
                height,
                format_code,
                tags::USAGE_GPU_SAMPLED_IMAGE,
                max_images,
                &mut reader,
            )
        };
        let Some(reader) = NonNull::new(reader).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            return Err(media_backend_failure("AImageReader_newWithUsage", status));
        };

        // С этого места reader принадлежит `Self`: при любой ошибке ниже его удалит Drop.
        let mut window: *mut ndk::ANativeWindow = ptr::null_mut();
        // SAFETY: reader жив, `window` — валидное выходное место; окно принадлежит reader-у.
        let status = unsafe { ndk::AImageReader_getWindow(reader.as_ptr(), &mut window) };
        let context = Arc::new(ReaderContext::new(diagnostics));
        let Some(window) = NonNull::new(window).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            // SAFETY: reader создан выше и больше никому не принадлежит; слушатель ещё не ставился.
            unsafe { ndk::AImageReader_delete(reader.as_ptr()) };
            return Err(media_backend_failure("AImageReader_getWindow", status));
        };
        let sink = Self {
            reader,
            window,
            context,
        };

        let mut listener = ndk::AImageReader_ImageListener {
            context: sink.context.as_callback_context(),
            onImageAvailable: Some(reader_on_image_available),
        };
        // SAFETY: reader жив; NDK копирует структуру слушателя; контекст живёт до `AImageReader_delete` (Drop).
        let status = unsafe { ndk::AImageReader_setImageListener(sink.reader.as_ptr(), &mut listener) };
        if status != tags::MEDIA_STATUS_OK {
            return Err(media_backend_failure("AImageReader_setImageListener", status));
        }
        Ok(sink)
    }

    /// Окно, в которое камера пишет кадры (принадлежит reader-у и живёт столько же).
    pub(super) fn window(&self) -> *mut ndk::ANativeWindow {
        self.window.as_ptr()
    }
}

impl Drop for TemporaryRawSink {
    fn drop(&mut self) {
        // Сначала закрываем ворота слушателя: возврат значит, что ни один колбэк не внутри NDK, а
        // новые туда не заходят. Без этого `AImageReader_delete` (держит замок reader-а, ожидая
        // остановки looper-а) взаимоблокируется с колбэком в `acquireNextImage` — см. `reader_callback`.
        // Затем удаляем reader: деструктор NDK дожидается остановки looper-а, после чего контекст
        // можно освобождать.
        self.context.close_gate();
        // SAFETY: reader создан `AImageReader_newWithUsage` и принадлежит только нам; камера к этому
        // моменту уже отсоединена от окна (порядок закрытия: сессия → устройство → reader).
        unsafe { ndk::AImageReader_delete(self.reader.as_ptr()) };
    }
}
