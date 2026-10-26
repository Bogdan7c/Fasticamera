//! `ACameraDevice`: открытие и закрытие устройства.

use std::ffi::{CString, c_void};
use std::ptr::{self, NonNull};
use std::sync::Arc;

use camera_contract::ids::CameraId;
use camera_core::backend::{BackendEventSink, DeviceGeneration, OpenFailure};
use camera_ndk_sys as ndk;

use crate::diagnostics::BackendDiagnostics;
use crate::status_mapping::{camera_backend_failure, open_failure};
use crate::tags;

use super::callbacks::{CaptureContext, DeviceContext, device_on_disconnected, device_on_error};
use super::manager::CameraManager;

/// Открытое устройство камеры.
///
/// Владеет контекстом колбэков устройства и «ареной» контекстов результатов запросов всех
/// сессий этого устройства: они нужны NDK до остановки его looper-а, то есть до возврата
/// `ACameraDevice_close` (доказательство — в доке `callbacks`). Поля освобождаются **после**
/// тела `Drop`, то есть после закрытия устройства.
pub(super) struct OpenedDevice {
    raw: NonNull<ndk::ACameraDevice>,
    camera: CameraId,
    context: Arc<DeviceContext>,
    capture_contexts: Vec<Arc<CaptureContext>>,
}

impl OpenedDevice {
    /// Открывает камеру; события устройства помечаются `generation`.
    pub(super) fn open(
        manager: &CameraManager,
        camera: &CameraId,
        generation: DeviceGeneration,
        sink: BackendEventSink,
        diagnostics: Arc<BackendDiagnostics>,
    ) -> Result<Self, OpenFailure> {
        let c_id = CString::new(camera.as_str()).map_err(|_| {
            OpenFailure::Backend(camera_backend_failure(
                "ACameraManager_openCamera",
                tags::STATUS_INVALID_PARAMETER,
            ))
        })?;
        let context = Arc::new(DeviceContext::new(sink, generation, diagnostics));
        let mut callbacks = ndk::ACameraDevice_StateCallbacks {
            context: context.as_callback_context().cast::<c_void>(),
            onDisconnected: Some(device_on_disconnected),
            onError: Some(device_on_error),
            onClientSharedAccessPriorityChanged: None,
        };
        let mut raw: *mut ndk::ACameraDevice = ptr::null_mut();
        // SAFETY: менеджер жив, `c_id` — NUL-терминированная строка, NDK копирует структуру колбэков,
        // контекст живёт до закрытия устройства (поле `Self`), `raw` — валидное выходное место.
        let status =
            unsafe { ndk::ACameraManager_openCamera(manager.as_ptr(), c_id.as_ptr(), &mut callbacks, &mut raw) };
        match NonNull::new(raw) {
            Some(raw) if status == tags::STATUS_OK => Ok(Self {
                raw,
                camera: camera.clone(),
                context,
                capture_contexts: Vec::new(),
            }),
            _ => Err(open_failure("ACameraManager_openCamera", status)),
        }
    }

    pub(super) fn as_ptr(&self) -> *mut ndk::ACameraDevice {
        self.raw.as_ptr()
    }

    pub(super) fn camera(&self) -> &CameraId {
        &self.camera
    }

    /// Передаёт устройству контекст результатов сессии: он будет жить до закрытия устройства.
    pub(super) fn adopt_capture_context(&mut self, context: Arc<CaptureContext>) {
        self.capture_contexts.push(context);
    }
}

impl Drop for OpenedDevice {
    fn drop(&mut self) {
        // Сначала запрещаем события этого поколения, затем закрываем: `ACameraDevice_close` ждёт
        // остановки looper-а колбэков, поэтому запоздалые вызовы либо отработают до возврата (и будут
        // проигнорированы), либо не придут вовсе.
        self.context.mark_closed();
        for capture in &self.capture_contexts {
            capture.mark_closed();
        }
        // SAFETY: устройство открыто нами и принадлежит только нам; сессии на нём уже закрыты.
        let status = unsafe { ndk::ACameraDevice_close(self.raw.as_ptr()) };
        if status != tags::STATUS_OK {
            tracing::warn!(
                target: "fasticamera::camera::device",
                camera = %self.camera,
                status,
                "ACameraDevice_close вернул ошибку"
            );
        }
    }
}
