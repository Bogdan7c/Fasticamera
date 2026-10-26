//! Колбэки NDK → события бэкенда. Safety-модуль №1 из двух, где разрешён `extern "C"`.
//!
//! Правила каждого колбэка (они приходят на чужих потоках NDK, а не на потоке камеры):
//! * тело под [`std::panic::catch_unwind`]: паника не должна пересечь границу C — она считается
//!   в диагностике и логируется;
//! * ничего не блокирует: разобрать аргументы, отправить [`BackendEvent`] через
//!   [`BackendEventSink`] (его `deliver` по контракту не ждёт) и вернуться;
//! * после закрытия устройства/сессии (флаг `closed` у контекста) событие не отправляется, а
//!   считается в `callbacks_after_close`: запоздалое «отключено» не должно убить новое устройство.
//!
//! # Время жизни контекстов (главный риск use-after-free)
//!
//! Контекст — `Arc<…>`, указатель на него (`Arc::as_ptr`) отдан NDK. Что доказано исходниками AOSP
//! (`frameworks/av`, ветка main) и когда освобождать:
//!
//! * **Устройство и результаты запросов.** `ACameraDevice_close` делает `delete device`
//!   (`camera/ndk/NdkCameraDevice.cpp`), деструктор зовёт `stopLooperAndDisconnect()`
//!   (`camera/ndk/impl/ACameraDevice.cpp`): `mCbLooper->unregisterHandler(..); mCbLooper->stop()`,
//!   а `ALooper::stop()` ждёт завершения потока (`thread->requestExitAndWait()`,
//!   `media/module/foundation/ALooper.cpp`). Колбэки устройства и запросов идут **только** через
//!   этот looper (`kWhatOnDisconnected`, `kWhatOnError`, `kWhatCapture*`), значит после возврата
//!   `ACameraDevice_close` ни один из них больше не вызовется. Эти контексты освобождаются
//!   вместе с устройством, после `ACameraDevice_close`.
//! * **Состояние сессии.** `onReady`/`onActive` идут через тот же looper, но `onClosed` вызывается
//!   ещё и **прямо из деструктора** `ACameraCaptureSession` (`camera/ndk/impl/ACameraCaptureSession.cpp`),
//!   а он может отработать на любом потоке и позже закрытия устройства (последняя ссылка на сессию
//!   живёт в сообщениях looper и кэше обработчика). Доказать, что деструктор отработает до возврата
//!   `ACameraDevice_close`, нельзя, поэтому контексты состояния сессий **не освобождаются до конца
//!   жизни бэкенда** (около 100 байт на сессию; считаются в `retained_session_contexts`), а при
//!   разрушении самого бэкенда **намеренно утекают** (`context_leak`, `leaked_session_state_contexts`):
//!   деструктор сессии может отработать и после `Drop` бэкенда.
//! * **Доступность камер.** `ACameraManager_unregisterAvailabilityCallback` ждёт доставленные колбэки
//!   (`drainPendingCallbacksLocked`, пауза 5 мс за шаг) и при ошибке ожидания выходит, не дождавшись
//!   (`camera/ndk/impl/ACameraManager.cpp`). Гарантии нет, поэтому контекст доступности
//!   **намеренно утекает** (один на запуск бэкенда, `leaked_availability_contexts`), а после снятия
//!   колбэка лишь перестаёт слать события.
//! * **Слушатель `AImageReader`.** Деструктор reader-а останавливает свой looper тем же `stop()`
//!   (`media/ndk/NdkImageReader.cpp`): контекст освобождается после `AImageReader_delete`. Но сам
//!   `AImageReader::close()` держит внутренний замок `mLock` (строки ~380–397), пока ждёт остановки
//!   looper-а, а колбэк внутри `acquireNextImage` ждёт тот же замок: без «ворот»
//!   (`callback_gate`) закрытие reader-а повисло бы навсегда. Поэтому `delete` вызывается только
//!   после `CallbackGate::close`.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_contract::capture_result::FrameNumber;
use camera_contract::ids::CameraId;
use camera_core::backend::{BackendEvent, BackendEventSink, CameraAvailability, DeviceGeneration, SessionGeneration};
use camera_ndk_sys as ndk;

use crate::diagnostics::BackendDiagnostics;
use crate::frame_numbers::{FrameNumberMatcher, FrameNumberSource};
use crate::result_meta::{parse_capture_result, read_sensor_timestamp};
use crate::status_mapping::{capture_failure_reason, device_error_kind};

use super::manager::MetadataView;

const LOG_TARGET: &str = "fasticamera::camera::callbacks";

/// Выполняет тело колбэка под защитой от паники; паника учитывается, но дальше не идёт.
fn run_guarded(diagnostics: &BackendDiagnostics, callback: &'static str, body: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        diagnostics.record_callback_panic();
        tracing::error!(target: "fasticamera::camera::callbacks", callback, "паника в колбэке NDK перехвачена на границе C");
    }
}

/// Превращает `void* context` обратно в ссылку на контекст.
///
/// # Safety
/// `context` — указатель из `Arc::as_ptr` контекста типа `T`, который жив на всё время колбэка
/// (см. правила освобождения в доке модуля) или `null`.
unsafe fn context_ref<'a, T>(context: *mut c_void) -> Option<&'a T> {
    // SAFETY: по контракту функции указатель либо null, либо ведёт на живой `T`.
    unsafe { context.cast_const().cast::<T>().as_ref() }
}

// ---------------------------------------------------------------------------------------------
// Устройство
// ---------------------------------------------------------------------------------------------

/// Контекст колбэков устройства. Освобождается после `ACameraDevice_close`.
pub(super) struct DeviceContext {
    sink: BackendEventSink,
    generation: DeviceGeneration,
    diagnostics: Arc<BackendDiagnostics>,
    closed: AtomicBool,
}

impl DeviceContext {
    pub(super) fn new(
        sink: BackendEventSink,
        generation: DeviceGeneration,
        diagnostics: Arc<BackendDiagnostics>,
    ) -> Self {
        Self {
            sink,
            generation,
            diagnostics,
            closed: AtomicBool::new(false),
        }
    }

    /// Устройство закрывается: дальше события этого поколения не отправляются.
    pub(super) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(super) fn as_callback_context(self: &Arc<Self>) -> *mut c_void {
        Arc::as_ptr(self).cast_mut().cast()
    }

    /// Отправляет событие, если устройство не закрыто; иначе считает опоздавший колбэк.
    fn deliver_unless_closed(&self, event: BackendEvent) {
        if self.closed.load(Ordering::Acquire) {
            self.diagnostics.record_callback_after_close();
        } else {
            self.sink.deliver(event);
        }
    }
}

/// `ACameraDevice_StateCallbacks::onDisconnected`.
pub(super) unsafe extern "C" fn device_on_disconnected(context: *mut c_void, _device: *mut ndk::ACameraDevice) {
    // SAFETY: context — `DeviceContext` из `OpenedDevice`, живой до конца `ACameraDevice_close`.
    let Some(context) = (unsafe { context_ref::<DeviceContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "device_on_disconnected", || {
        context.deliver_unless_closed(BackendEvent::DeviceDisconnected {
            device: context.generation,
        });
    });
}

/// `ACameraDevice_StateCallbacks::onError`.
pub(super) unsafe extern "C" fn device_on_error(context: *mut c_void, _device: *mut ndk::ACameraDevice, error: c_int) {
    // SAFETY: см. `device_on_disconnected`.
    let Some(context) = (unsafe { context_ref::<DeviceContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "device_on_error", || {
        context.deliver_unless_closed(BackendEvent::DeviceError {
            device: context.generation,
            error: device_error_kind(error),
        });
    });
}

// ---------------------------------------------------------------------------------------------
// Состояние сессии
// ---------------------------------------------------------------------------------------------

/// Контекст колбэков состояния сессии. Не освобождается до конца жизни бэкенда
/// (`onClosed` может прийти из деструктора сессии после закрытия устройства).
pub(super) struct SessionStateContext {
    sink: BackendEventSink,
    generation: SessionGeneration,
    diagnostics: Arc<BackendDiagnostics>,
    closed_by_backend: AtomicBool,
}

impl SessionStateContext {
    pub(super) fn new(
        sink: BackendEventSink,
        generation: SessionGeneration,
        diagnostics: Arc<BackendDiagnostics>,
    ) -> Self {
        Self {
            sink,
            generation,
            diagnostics,
            closed_by_backend: AtomicBool::new(false),
        }
    }

    /// Бэкенд сам закрывает сессию: эхо `onClosed` не нужно (движок знает, что закрыл).
    pub(super) fn mark_closed_by_backend(&self) {
        self.closed_by_backend.store(true, Ordering::Release);
    }

    pub(super) fn as_callback_context(self: &Arc<Self>) -> *mut c_void {
        Arc::as_ptr(self).cast_mut().cast()
    }
}

/// `ACameraCaptureSession_stateCallbacks::onClosed`.
pub(super) unsafe extern "C" fn session_on_closed(context: *mut c_void, _session: *mut ndk::ACameraCaptureSession) {
    // SAFETY: context — `SessionStateContext`, который не освобождается до конца жизни бэкенда.
    let Some(context) = (unsafe { context_ref::<SessionStateContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "session_on_closed", || {
        if context.closed_by_backend.load(Ordering::Acquire) {
            context.diagnostics.record_callback_after_close();
        } else {
            context.sink.deliver(BackendEvent::SessionClosed {
                session: context.generation,
            });
        }
    });
}

/// `onReady`/`onActive`: события движку не нужны, но колбэк обязан быть валидным и не паниковать.
pub(super) unsafe extern "C" fn session_on_idle_transition(
    context: *mut c_void,
    _session: *mut ndk::ACameraCaptureSession,
) {
    // SAFETY: см. `session_on_closed`.
    let Some(context) = (unsafe { context_ref::<SessionStateContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "session_on_idle_transition", || {
        tracing::trace!(target: LOG_TARGET, session = context.generation.0, "переход готов/активен сессии");
    });
}

// ---------------------------------------------------------------------------------------------
// Результаты запросов (колбэки V2, API 33)
// ---------------------------------------------------------------------------------------------

/// Контекст колбэков результатов. Освобождается вместе с устройством, после `ACameraDevice_close`
/// (колбэки запросов идут только через looper устройства).
pub(super) struct CaptureContext {
    sink: BackendEventSink,
    generation: SessionGeneration,
    diagnostics: Arc<BackendDiagnostics>,
    closed: AtomicBool,
    matcher: Mutex<FrameNumberMatcher>,
}

impl CaptureContext {
    pub(super) fn new(
        sink: BackendEventSink,
        generation: SessionGeneration,
        diagnostics: Arc<BackendDiagnostics>,
        frame_number_window: usize,
    ) -> Self {
        Self {
            sink,
            generation,
            diagnostics,
            closed: AtomicBool::new(false),
            matcher: Mutex::new(FrameNumberMatcher::new(frame_number_window)),
        }
    }

    pub(super) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub(super) fn as_callback_context(self: &Arc<Self>) -> *mut c_void {
        Arc::as_ptr(self).cast_mut().cast()
    }

    fn matcher(&self) -> MutexGuard<'_, FrameNumberMatcher> {
        // Паника другого колбэка под замком не должна глушить все следующие результаты.
        self.matcher.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `captureCallbacksV2::onCaptureStarted`: запоминает номер кадра по метке сенсора.
pub(super) unsafe extern "C" fn capture_on_started(
    context: *mut c_void,
    _session: *mut ndk::ACameraCaptureSession,
    _request: *const ndk::ACaptureRequest,
    timestamp: i64,
    frame_number: i64,
) {
    // SAFETY: context — `CaptureContext`, живой до конца `ACameraDevice_close`.
    let Some(context) = (unsafe { context_ref::<CaptureContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "capture_on_started", || {
        context.matcher().record_started(
            camera_contract::capture_result::SensorTimestamp::from_nanos(timestamp),
            FrameNumber(frame_number),
        );
    });
}

/// `captureCallbacksV2::onCaptureCompleted`: результат кадра → `CaptureCompleted`.
pub(super) unsafe extern "C" fn capture_on_completed(
    context: *mut c_void,
    _session: *mut ndk::ACameraCaptureSession,
    _request: *mut ndk::ACaptureRequest,
    result: *const ndk::ACameraMetadata,
) {
    // SAFETY: см. `capture_on_started`.
    let Some(context) = (unsafe { context_ref::<CaptureContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "capture_on_completed", || {
        if context.closed.load(Ordering::Acquire) {
            context.diagnostics.record_callback_after_close();
            return;
        }
        // SAFETY: `result` валиден на время колбэка; `MetadataView` не переживает эту функцию.
        let Some(view) = (unsafe { MetadataView::from_raw(result) }) else {
            return;
        };
        let source = context.matcher().resolve(read_sensor_timestamp(&view));
        if let FrameNumberSource::Fallback(_) = source {
            context.diagnostics.record_unmatched_frame_number();
        }
        let parsed = parse_capture_result(&view, source.frame_number());
        context.sink.deliver(BackendEvent::CaptureCompleted {
            session: context.generation,
            result: parsed.meta,
        });
    });
}

/// `captureCallbacksV2::onCaptureFailed`: провал кадра → `CaptureFailed`.
pub(super) unsafe extern "C" fn capture_on_failed(
    context: *mut c_void,
    _session: *mut ndk::ACameraCaptureSession,
    _request: *mut ndk::ACaptureRequest,
    failure: *mut ndk::ACameraCaptureFailure,
) {
    // SAFETY: см. `capture_on_started`.
    let Some(context) = (unsafe { context_ref::<CaptureContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "capture_on_failed", || {
        if context.closed.load(Ordering::Acquire) {
            context.diagnostics.record_callback_after_close();
            return;
        }
        // SAFETY: `failure` валиден на время колбэка (указатель из NDK).
        let Some(failure) = (unsafe { failure.as_ref() }) else {
            return;
        };
        context.sink.deliver(BackendEvent::CaptureFailed {
            session: context.generation,
            frame_number: FrameNumber(failure.frameNumber),
            reason: capture_failure_reason(failure.reason),
        });
    });
}

// ---------------------------------------------------------------------------------------------
// Доступность камер
// ---------------------------------------------------------------------------------------------

/// Контекст колбэков доступности камер. Намеренно живёт до конца процесса (см. доку модуля);
/// после снятия колбэка событий не шлёт.
pub(super) struct AvailabilityContext {
    sink: Mutex<Option<BackendEventSink>>,
    diagnostics: Arc<BackendDiagnostics>,
}

impl AvailabilityContext {
    pub(super) fn new(sink: BackendEventSink, diagnostics: Arc<BackendDiagnostics>) -> Self {
        Self {
            sink: Mutex::new(Some(sink)),
            diagnostics,
        }
    }

    /// Колбэк снят: больше ничего не отправляем и отпускаем ручку событий.
    pub(super) fn retire(&self) {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub(super) fn as_callback_context(self: &Arc<Self>) -> *mut c_void {
        Arc::as_ptr(self).cast_mut().cast()
    }

    fn deliver(&self, camera_id: *const c_char, availability: CameraAvailability) {
        let sink = self.sink.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let Some(sink) = sink else {
            self.diagnostics.record_callback_after_close();
            return;
        };
        if camera_id.is_null() {
            return;
        }
        // SAFETY: NDK передаёт NUL-терминированную строку, живую на время колбэка; null отсечён выше.
        let text = unsafe { CStr::from_ptr(camera_id) };
        let Ok(id) = text.to_str() else {
            self.diagnostics
                .record_failure("availability_callback", 0, "ID камеры не UTF-8");
            return;
        };
        let Ok(camera) = CameraId::new(id) else {
            self.diagnostics
                .record_failure("availability_callback", 0, "пустой ID камеры");
            return;
        };
        sink.deliver(BackendEvent::CameraAvailability { camera, availability });
    }
}

/// `ACameraManager_AvailabilityCallbacks::onCameraAvailable`.
pub(super) unsafe extern "C" fn availability_on_available(context: *mut c_void, camera_id: *const c_char) {
    // SAFETY: context — `AvailabilityContext`, который никогда не освобождается.
    let Some(context) = (unsafe { context_ref::<AvailabilityContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "availability_on_available", || {
        context.deliver(camera_id, CameraAvailability::Available);
    });
}

/// `ACameraManager_AvailabilityCallbacks::onCameraUnavailable`.
pub(super) unsafe extern "C" fn availability_on_unavailable(context: *mut c_void, camera_id: *const c_char) {
    // SAFETY: см. `availability_on_available`.
    let Some(context) = (unsafe { context_ref::<AvailabilityContext>(context) }) else {
        return;
    };
    run_guarded(&context.diagnostics, "availability_on_unavailable", || {
        context.deliver(camera_id, CameraAvailability::Unavailable);
    });
}
