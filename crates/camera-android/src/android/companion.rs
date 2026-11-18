//! Вспомогательный поток 3A (решение 62): `AImageReader` YUV, чьё окно попадает в сессию как
//! выход, но **никогда не становится целью ни одного запроса**.
//!
//! Зачем: в сессии из одного RAW-потока Qualcomm не считает статистику 3A. Достаточно объявить
//! маленький YUV-поток и не слать в него запросов — автоматика HAL оживает, RAW-кадры и 60 к/с
//! сохраняются. Пиксели этого потока не читаются никогда (решение 8).
//!
//! # Время жизни
//!
//! Слушателя нет: образы сюда не приходят, поэтому не нужны ни `CallbackGate`, ни отсрочка
//! удаления (`RetiredReaders`). Окно читателя привязано к устройству так же, как окно RAW-читателя,
//! поэтому читатель уходит вместе с ним — через `ParkedOutputs` (см. `outputs`), а не при закрытии
//! `ACameraCaptureSession`. Если вопреки ожиданию образы всё же пришли, перед удалением их
//! забирают и сразу освобождают, а число идёт в диагностику (`companion_stray_images`).

use std::ptr::{self, NonNull};
use std::sync::Arc;

use camera_ndk_sys as ndk;

use crate::companion_stream::CompanionStreamSpec;
use crate::diagnostics::BackendDiagnostics;
use crate::tags;

/// Владелец вспомогательного читателя. `Drop` удаляет читателя вместе с его окном.
pub(super) struct CompanionReader {
    reader: NonNull<ndk::AImageReader>,
    window: NonNull<ndk::ANativeWindow>,
    /// Сколько образов максимум разбирать за один проход (защита от бесконечного цикла).
    max_images: u32,
    diagnostics: Arc<BackendDiagnostics>,
}

impl CompanionReader {
    /// Создаёт читателя YUV_420_888 заданного размера. `Err` — код `media_status_t`.
    pub(super) fn create(spec: &CompanionStreamSpec, diagnostics: Arc<BackendDiagnostics>) -> Result<Self, i32> {
        let (Ok(width), Ok(height), Ok(max_images)) = (
            i32::try_from(spec.size.width),
            i32::try_from(spec.size.height),
            i32::try_from(spec.max_images),
        ) else {
            return Err(tags::MEDIA_STATUS_UNKNOWN_ERROR);
        };
        let mut reader: *mut ndk::AImageReader = ptr::null_mut();
        // SAFETY: аргументы — обычные числа, `reader` — валидное выходное место.
        let status =
            unsafe { ndk::AImageReader_new(width, height, tags::IMAGE_FORMAT_YUV_420_888, max_images, &mut reader) };
        let Some(reader) = NonNull::new(reader).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            return Err(failure_status(status));
        };
        let mut window: *mut ndk::ANativeWindow = ptr::null_mut();
        // SAFETY: читатель жив, `window` — валидное выходное место; окно принадлежит читателю.
        let status = unsafe { ndk::AImageReader_getWindow(reader.as_ptr(), &mut window) };
        let Some(window) = NonNull::new(window).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            // SAFETY: читатель только что создан нами, слушателя и образов нет, окно не отдавалось.
            unsafe { ndk::AImageReader_delete(reader.as_ptr()) };
            return Err(failure_status(status));
        };
        Ok(Self {
            reader,
            window,
            max_images: spec.max_images,
            diagnostics,
        })
    }

    /// Окно для контейнера выходов сессии. Живёт, пока жив читатель.
    pub(super) fn window(&self) -> *mut ndk::ANativeWindow {
        self.window.as_ptr()
    }

    /// Забирает и освобождает образы, которых здесь быть не должно, и считает их. Зовётся при
    /// закрытии сессии, когда камера уже не пишет в окно.
    pub(super) fn discard_stray_images(&self) {
        let mut discarded: u64 = 0;
        for _ in 0..self.max_images {
            let mut image: *mut ndk::AImage = ptr::null_mut();
            // SAFETY: читатель жив; `image` — валидное выходное место.
            let status = unsafe { ndk::AImageReader_acquireNextImage(self.reader.as_ptr(), &mut image) };
            if status != tags::MEDIA_STATUS_OK {
                if status != tags::MEDIA_STATUS_NO_BUFFER_AVAILABLE {
                    self.diagnostics.record_failure(
                        "AImageReader_acquireNextImage(companion)",
                        status,
                        "не удалось проверить вспомогательный поток на лишние образы",
                    );
                }
                break;
            }
            let Some(image) = NonNull::new(image) else {
                break;
            };
            // SAFETY: образ получен выше и нигде не сохранён; его пиксели не читались.
            unsafe { ndk::AImage_delete(image.as_ptr()) };
            discarded += 1;
        }
        if discarded > 0 {
            self.diagnostics.record_companion_stray_images(discarded);
        }
    }
}

impl Drop for CompanionReader {
    fn drop(&mut self) {
        // SAFETY: читатель создан нами и принадлежит только нам; слушателя нет, образы возвращены
        // (`discard_stray_images`) или не выдавались; камера отсоединилась от окна, потому что
        // удаление идёт в том же порядке, что у RAW-читателя (см. доку модуля).
        unsafe { ndk::AImageReader_delete(self.reader.as_ptr()) };
    }
}

/// Код отказа: `OK` без читателя/окна — тоже отказ.
fn failure_status(status: i32) -> i32 {
    if status == tags::MEDIA_STATUS_OK {
        tags::MEDIA_STATUS_UNKNOWN_ERROR
    } else {
        status
    }
}
