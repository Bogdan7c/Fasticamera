//! Регистрация колбэков доступности камер (`ACameraManager_registerAvailabilityCallback`).

use std::sync::Arc;

use camera_core::backend::{BackendEventSink, BackendFailure};
use camera_ndk_sys as ndk;

use crate::diagnostics::BackendDiagnostics;
use crate::status_mapping::camera_backend_failure;
use crate::tags;

use super::callbacks::{AvailabilityContext, availability_on_available, availability_on_unavailable};
use super::manager::CameraManager;

/// Зарегистрированные колбэки доступности. Снимаются в [`AvailabilityRegistration::unregister`].
pub(super) struct AvailabilityRegistration {
    callbacks: ndk::ACameraManager_AvailabilityCallbacks,
    context: Arc<AvailabilityContext>,
}

impl AvailabilityRegistration {
    pub(super) fn register(
        manager: &CameraManager,
        sink: BackendEventSink,
        diagnostics: Arc<BackendDiagnostics>,
    ) -> Result<Self, BackendFailure> {
        let context = Arc::new(AvailabilityContext::new(sink, diagnostics));
        let callbacks = ndk::ACameraManager_AvailabilityCallbacks {
            context: context.as_callback_context(),
            onCameraAvailable: Some(availability_on_available),
            onCameraUnavailable: Some(availability_on_unavailable),
        };
        // SAFETY: менеджер жив; NDK копирует структуру; контекст никогда не освобождается (см. `unregister`).
        let status = unsafe { ndk::ACameraManager_registerAvailabilityCallback(manager.as_ptr(), &callbacks) };
        if status != tags::STATUS_OK {
            return Err(camera_backend_failure(
                "ACameraManager_registerAvailabilityCallback",
                status,
            ));
        }
        Ok(Self { callbacks, context })
    }

    /// Снимает колбэки. Контекст **намеренно утекает**: NDK не гарантирует отсутствие запоздалых
    /// вызовов после снятия (подробности — в доке `callbacks`), поэтому память остаётся валидной,
    /// а события прекращаются через `retire`.
    pub(super) fn unregister(self, manager: &CameraManager, diagnostics: &BackendDiagnostics) {
        // SAFETY: менеджер жив; передаём структуру с теми же значениями, что при регистрации.
        let status = unsafe { ndk::ACameraManager_unregisterAvailabilityCallback(manager.as_ptr(), &self.callbacks) };
        if status != tags::STATUS_OK {
            diagnostics.record_failure(
                "ACameraManager_unregisterAvailabilityCallback",
                status,
                "не удалось снять колбэк",
            );
        }
        self.context.retire();
        diagnostics.record_leaked_availability_context();
        std::mem::forget(self.context);
    }
}
