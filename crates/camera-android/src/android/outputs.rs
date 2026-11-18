//! Выходы сессии: цель запроса, контейнер выходов и «припаркованные» читатели закрытой сессии.

use std::ptr::{self, NonNull};

use camera_core::backend::BackendFailure;
use camera_ndk_sys as ndk;

use crate::status_mapping::camera_backend_failure;
use crate::tags;

use super::companion::CompanionReader;
use super::raw_reader::RawReader;

/// Цель запроса (`ACameraOutputTarget`) на окно приёмника.
pub(super) struct OutputTarget {
    raw: NonNull<ndk::ACameraOutputTarget>,
}

impl OutputTarget {
    pub(super) fn create(window: *mut ndk::ANativeWindow) -> Result<Self, BackendFailure> {
        let mut raw: *mut ndk::ACameraOutputTarget = ptr::null_mut();
        // SAFETY: окно принадлежит живому reader-у; `raw` — валидное выходное место.
        let status = unsafe { ndk::ACameraOutputTarget_create(window, &mut raw) };
        match NonNull::new(raw) {
            Some(raw) if status == tags::STATUS_OK => Ok(Self { raw }),
            _ => Err(camera_backend_failure("ACameraOutputTarget_create", status)),
        }
    }

    pub(in crate::android) fn as_ptr(&self) -> *const ndk::ACameraOutputTarget {
        self.raw.as_ptr().cast_const()
    }
}

impl Drop for OutputTarget {
    fn drop(&mut self) {
        // SAFETY: цель создана нами; запросы, ссылавшиеся на неё, уже освобождены.
        unsafe { ndk::ACameraOutputTarget_free(self.raw.as_ptr()) };
    }
}

/// Контейнер выходов сессии: нужен только на время создания сессии. Выходов может быть несколько
/// (RAW и вспомогательный поток); целью запросов становится только RAW — отдельно, `OutputTarget`.
pub(super) struct OutputContainer {
    container: NonNull<ndk::ACaptureSessionOutputContainer>,
    outputs: Vec<NonNull<ndk::ACaptureSessionOutput>>,
}

impl OutputContainer {
    /// Пустой контейнер.
    pub(super) fn create() -> Result<Self, BackendFailure> {
        let mut container: *mut ndk::ACaptureSessionOutputContainer = ptr::null_mut();
        // SAFETY: `container` — валидное выходное место.
        let status = unsafe { ndk::ACaptureSessionOutputContainer_create(&mut container) };
        match NonNull::new(container) {
            Some(container) if status == tags::STATUS_OK => Ok(Self {
                container,
                outputs: Vec::new(),
            }),
            _ => Err(camera_backend_failure("ACaptureSessionOutputContainer_create", status)),
        }
    }

    /// Добавляет в контейнер выход на окно. Окно должно пережить создание сессии.
    pub(super) fn add_window(&mut self, window: *mut ndk::ANativeWindow) -> Result<(), BackendFailure> {
        let mut output: *mut ndk::ACaptureSessionOutput = ptr::null_mut();
        // SAFETY: окно принадлежит живому reader-у; `output` — валидное выходное место.
        let status = unsafe { ndk::ACaptureSessionOutput_create(window, &mut output) };
        let Some(output) = NonNull::new(output).filter(|_| status == tags::STATUS_OK) else {
            return Err(camera_backend_failure("ACaptureSessionOutput_create", status));
        };
        // Выход принадлежит контейнеру-владельцу с этого места: Drop освободит его и при ошибке ниже.
        self.outputs.push(output);
        // SAFETY: контейнер и выход живы.
        let status = unsafe { ndk::ACaptureSessionOutputContainer_add(self.container.as_ptr(), output.as_ptr()) };
        if status == tags::STATUS_OK {
            Ok(())
        } else {
            Err(camera_backend_failure("ACaptureSessionOutputContainer_add", status))
        }
    }

    pub(super) fn as_ptr(&self) -> *const ndk::ACaptureSessionOutputContainer {
        self.container.as_ptr().cast_const()
    }
}

impl Drop for OutputContainer {
    fn drop(&mut self) {
        // SAFETY: контейнер и выходы созданы нами; сессия уже создана (или не создана) и копий не держит.
        unsafe {
            ndk::ACaptureSessionOutputContainer_free(self.container.as_ptr());
            for output in &self.outputs {
                ndk::ACaptureSessionOutput_free(output.as_ptr());
            }
        }
    }
}

/// Читатели закрытой сессии, ожидающие, пока устройство отпустит их окна.
///
/// Структура только держит владельцев: их `Drop` и есть действие. RAW-читатель уходит в отсрочку
/// (удалится, когда вернутся все его кадры), вспомогательный удаляется сразу — кадров у него нет.
pub(super) struct ParkedOutputs {
    _raw: RawReader,
    _companion: Option<CompanionReader>,
}

impl ParkedOutputs {
    /// `raw` уже остановлен (`stop_offering`); у вспомогательного потока проверяются лишние образы.
    pub(super) fn new(raw: RawReader, companion: Option<CompanionReader>) -> Self {
        if let Some(companion) = &companion {
            companion.discard_stray_images();
        }
        Self {
            _raw: raw,
            _companion: companion,
        }
    }
}
