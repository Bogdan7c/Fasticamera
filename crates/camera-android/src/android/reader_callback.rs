//! Слушатель временного RAW-приёмника. Safety-модуль №2 из двух, где разрешён `extern "C"`.
//!
//! ВРЕМЕННО до сессии 05: заменяется лизингом `RawFrameLease`. Слушатель забирает кадры
//! `AImageReader` и сразу отпускает их (`AImage_delete`); пиксели не читаются никогда.
//! Колбэк идёт на потоке looper-а самого `AImageReader`, не ждёт ничего, что держит поток камеры,
//! и не паникует через границу C (под `catch_unwind`). Порядок «ворота → забрать → отпустить» живёт
//! в нейтральных `reader_drain` и `callback_gate` (там же тесты гонки с закрытием).

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use camera_ndk_sys as ndk;

use crate::callback_gate::CallbackGate;
use crate::diagnostics::BackendDiagnostics;
use crate::reader_drain::{AcquireOutcome, ReaderOps, drain_available_images};
use crate::tags;

/// Контекст слушателя. Принадлежит `TemporaryRawSink` и освобождается после `AImageReader_delete`.
///
/// Два отдельных факта о времени жизни (оба по `media/ndk/NdkImageReader.cpp`, AOSP main):
/// * деструктор reader-а останавливает свой looper с ожиданием, поэтому после возврата
///   `AImageReader_delete` колбэк уже не придёт и контекст можно освобождать;
/// * но сам `delete` (`AImageReader::close()`, строки ~380–397) ждёт остановки looper-а, **удерживая
///   внутренний замок reader-а**, а колбэк внутри `AImageReader_acquireNextImage` ждёт тот же замок.
///   Поэтому перед `delete` обязательно закрываются ворота ([`ReaderContext::close_gate`]): после
///   их возврата ни один колбэк не внутри NDK, а новые в NDK не заходят.
pub(super) struct ReaderContext {
    diagnostics: Arc<BackendDiagnostics>,
    gate: CallbackGate,
}

impl ReaderContext {
    pub(super) fn new(diagnostics: Arc<BackendDiagnostics>) -> Self {
        Self {
            diagnostics,
            gate: CallbackGate::new(),
        }
    }

    /// Reader скоро удаляется: дождаться выхода вошедшего колбэка и не пускать новые.
    /// Вызывать **до** `AImageReader_delete`.
    pub(super) fn close_gate(&self) {
        self.gate.close();
    }

    pub(super) fn as_callback_context(self: &Arc<Self>) -> *mut c_void {
        Arc::as_ptr(self).cast_mut().cast()
    }
}

/// Операции NDK над живым reader-ом для нейтрального опустошения очереди.
struct NdkReaderOps {
    reader: *mut ndk::AImageReader,
}

impl ReaderOps for NdkReaderOps {
    type Image = *mut ndk::AImage;

    fn acquire_next_image(&self) -> AcquireOutcome<Self::Image> {
        let mut image: *mut ndk::AImage = ptr::null_mut();
        // SAFETY: reader жив на время колбэка (`delete` не начнётся, пока ворота удерживаются
        // колбэком, см. `close_gate`); `image` — валидное выходное место.
        let status = unsafe { ndk::AImageReader_acquireNextImage(self.reader, &mut image) };
        if status == tags::MEDIA_STATUS_NO_BUFFER_AVAILABLE {
            AcquireOutcome::NoBuffer
        } else if status != tags::MEDIA_STATUS_OK || image.is_null() {
            AcquireOutcome::Failed { status }
        } else {
            AcquireOutcome::Image(image)
        }
    }

    fn release_image(&self, image: Self::Image) {
        // SAFETY: `image` получен от `acquireNextImage` и ещё не освобождён.
        unsafe { ndk::AImage_delete(image) };
    }
}

/// `AImageReader_ImageListener::onImageAvailable`.
pub(super) unsafe extern "C" fn reader_on_image_available(context: *mut c_void, reader: *mut ndk::AImageReader) {
    // SAFETY: context — `ReaderContext` из `TemporaryRawSink`, живой до конца `AImageReader_delete`.
    let Some(context) = (unsafe { context.cast_const().cast::<ReaderContext>().as_ref() }) else {
        return;
    };
    let ops = NdkReaderOps { reader };
    let drained = catch_unwind(AssertUnwindSafe(|| {
        drain_available_images(&context.gate, &context.diagnostics, &ops)
    }));
    if drained.is_err() {
        context.diagnostics.record_callback_panic();
        tracing::error!(target: "fasticamera::camera::callbacks", "паника в слушателе AImageReader перехвачена");
    }
}
