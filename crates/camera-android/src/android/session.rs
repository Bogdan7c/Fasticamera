//! Сессия съёмки: контейнер выходов, цель запроса, сама `ACameraCaptureSession`,
//! повторяющийся запрос.
//!
//! # Порядок разрушения
//!
//! Стоп повтора → закрыть сессию → остановить приём кадров reader-а → (снаружи) закрыть устройство →
//! отложить reader (удаляется, когда все кадры вернулись, см. `raw_reader`). Окно
//! `AImageReader` не должно умереть раньше сессии, которая в него пишет. Поэтому поля
//! [`ActiveSession`] объявлены в порядке разрушения (Rust освобождает поля по порядку объявления):
//! сначала `handle` (стоп + закрытие сессии), затем `target`, затем `reader`. А при закрытии отдельно
//! от устройства reader не отбрасывается сразу, а возвращается вызывающему ([`ActiveSession::into_parked_reader`])
//! — отложить его можно только когда устройство отсоединилось от окна.

use std::ptr::{self, NonNull};
use std::sync::Arc;

use camera_contract::stream::{FrameRateMechanism, ResolvedStream};
use camera_core::backend::{BackendEventSink, BackendFailure, ConfigureFailure, RequestFailure, SessionGeneration};
use camera_ndk_sys as ndk;
use raw_frame_stream::RawFrameProducer;

use crate::diagnostics::BackendDiagnostics;
use crate::request_plan::{RequestPlan, apply_entries};
use crate::status_mapping::{camera_backend_failure, configure_failure, request_failure};
use crate::tags;
use crate::vendor::VendorCapabilities;

use super::callbacks::{
    CaptureContext, CaptureContextSpec, SessionStateContext, capture_on_completed, capture_on_failed,
    capture_on_started, session_on_closed, session_on_idle_transition,
};
use super::device::OpenedDevice;
use super::raw_reader::RawReader;
use super::request::CaptureRequestHandle;

/// Цель запроса (`ACameraOutputTarget`) на окно приёмника.
pub(super) struct OutputTarget {
    raw: NonNull<ndk::ACameraOutputTarget>,
}

impl OutputTarget {
    fn create(window: *mut ndk::ANativeWindow) -> Result<Self, BackendFailure> {
        let mut raw: *mut ndk::ACameraOutputTarget = ptr::null_mut();
        // SAFETY: окно принадлежит живому reader-у; `raw` — валидное выходное место.
        let status = unsafe { ndk::ACameraOutputTarget_create(window, &mut raw) };
        match NonNull::new(raw) {
            Some(raw) if status == tags::STATUS_OK => Ok(Self { raw }),
            _ => Err(camera_backend_failure("ACameraOutputTarget_create", status)),
        }
    }

    pub(super) fn as_ptr(&self) -> *const ndk::ACameraOutputTarget {
        self.raw.as_ptr().cast_const()
    }
}

impl Drop for OutputTarget {
    fn drop(&mut self) {
        // SAFETY: цель создана нами; запросы, ссылавшиеся на неё, уже освобождены.
        unsafe { ndk::ACameraOutputTarget_free(self.raw.as_ptr()) };
    }
}

/// Контейнер выходов с одним выходом: нужен только на время создания сессии.
struct OutputContainer {
    container: NonNull<ndk::ACaptureSessionOutputContainer>,
    output: NonNull<ndk::ACaptureSessionOutput>,
}

impl OutputContainer {
    fn with_window(window: *mut ndk::ANativeWindow) -> Result<Self, BackendFailure> {
        let mut container: *mut ndk::ACaptureSessionOutputContainer = ptr::null_mut();
        // SAFETY: `container` — валидное выходное место.
        let status = unsafe { ndk::ACaptureSessionOutputContainer_create(&mut container) };
        let Some(container) = NonNull::new(container).filter(|_| status == tags::STATUS_OK) else {
            return Err(camera_backend_failure("ACaptureSessionOutputContainer_create", status));
        };

        let mut output: *mut ndk::ACaptureSessionOutput = ptr::null_mut();
        // SAFETY: окно принадлежит живому reader-у; `output` — валидное выходное место.
        let status = unsafe { ndk::ACaptureSessionOutput_create(window, &mut output) };
        let Some(output) = NonNull::new(output).filter(|_| status == tags::STATUS_OK) else {
            // SAFETY: контейнер создан выше и никому не передан.
            unsafe { ndk::ACaptureSessionOutputContainer_free(container.as_ptr()) };
            return Err(camera_backend_failure("ACaptureSessionOutput_create", status));
        };

        // С этого места оба объекта принадлежат `Self` и освободятся его Drop.
        let this = Self { container, output };
        // SAFETY: контейнер и выход живы.
        let status = unsafe { ndk::ACaptureSessionOutputContainer_add(this.container.as_ptr(), this.output.as_ptr()) };
        if status != tags::STATUS_OK {
            return Err(camera_backend_failure("ACaptureSessionOutputContainer_add", status));
        }
        Ok(this)
    }
}

impl Drop for OutputContainer {
    fn drop(&mut self) {
        // SAFETY: контейнер и выход созданы нами; сессия уже создана (или не создана) и копий не держит.
        unsafe {
            ndk::ACaptureSessionOutputContainer_free(self.container.as_ptr());
            ndk::ACaptureSessionOutput_free(self.output.as_ptr());
        }
    }
}

/// Владелец `ACameraCaptureSession`: при разрушении останавливает повтор и закрывает сессию.
struct SessionHandle {
    raw: NonNull<ndk::ACameraCaptureSession>,
    state_context: Arc<SessionStateContext>,
    capture_context: Arc<CaptureContext>,
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        // Эхо `onClosed` нам не нужно (движок знает, что закрыл), а результаты закрытой сессии
        // больше не отправляем.
        self.state_context.mark_closed_by_backend();
        self.capture_context.mark_closed();
        // SAFETY: сессия создана нами и принадлежит только нам; устройство ещё открыто (закрывается позже).
        let status = unsafe { ndk::ACameraCaptureSession_stopRepeating(self.raw.as_ptr()) };
        if status != tags::STATUS_OK && status != tags::STATUS_SESSION_CLOSED {
            tracing::debug!(target: "fasticamera::camera::session", status, "stopRepeating при закрытии сессии вернул ошибку");
        }
        // SAFETY: см. выше; после `ACameraCaptureSession_close` указатель сессии не используется.
        unsafe { ndk::ACameraCaptureSession_close(self.raw.as_ptr()) };
    }
}

/// Активная сессия с приёмником и целью.
///
/// Порядок полей = порядок разрушения (см. доку модуля): `handle` → `target` → `reader`.
pub(super) struct ActiveSession {
    handle: SessionHandle,
    target: OutputTarget,
    reader: RawReader,
    generation: SessionGeneration,
    stream: ResolvedStream,
    mechanism: FrameRateMechanism,
}

/// Всё, что нужно для создания сессии.
pub(super) struct SessionSpec<'a> {
    pub stream: &'a ResolvedStream,
    pub mechanism: FrameRateMechanism,
    pub generation: SessionGeneration,
    /// Параметры сессии (публичные ключи рычага частоты); `None` — обычная сессия без параметров.
    pub session_parameters: Option<&'a RequestPlan>,
    pub vendor: &'a VendorCapabilities,
    pub frame_number_window: usize,
    /// Поток кадров: результаты запросов этой сессии уходят в него с эпохой её reader-а.
    pub producer: &'a RawFrameProducer,
}

impl ActiveSession {
    /// Создаёт сессию на открытом устройстве. На любой ошибке ничего не течёт: приёмник и
    /// вспомогательные объекты освобождаются RAII.
    pub(super) fn create(
        device: &mut OpenedDevice,
        reader: RawReader,
        spec: &SessionSpec<'_>,
        event_sink: &BackendEventSink,
        diagnostics: &Arc<BackendDiagnostics>,
    ) -> Result<(Self, Arc<SessionStateContext>), ConfigureFailure> {
        let window = reader.window();
        let backend_failure = |failure: BackendFailure| ConfigureFailure::Backend(failure);
        let outputs = OutputContainer::with_window(window).map_err(backend_failure)?;
        let target = OutputTarget::create(window).map_err(backend_failure)?;

        let state_context = Arc::new(SessionStateContext::new(
            event_sink.clone(),
            spec.generation,
            Arc::clone(diagnostics),
        ));
        let capture_context = Arc::new(CaptureContext::new(CaptureContextSpec {
            sink: event_sink.clone(),
            generation: spec.generation,
            diagnostics: Arc::clone(diagnostics),
            producer: spec.producer.clone(),
            epoch: reader.epoch(),
            frame_number_window: spec.frame_number_window,
        }));
        let state_callbacks = ndk::ACameraCaptureSession_stateCallbacks {
            context: state_context.as_callback_context(),
            onClosed: Some(session_on_closed),
            onReady: Some(session_on_idle_transition),
            onActive: Some(session_on_idle_transition),
        };

        let session_request = spec
            .session_parameters
            .map(|plan| build_request(device, plan, spec.vendor))
            .transpose()
            .map_err(backend_failure)?;
        let mut raw: *mut ndk::ACameraCaptureSession = ptr::null_mut();
        // SAFETY: устройство открыто; контейнер, параметры (если есть) и колбэки живы на время вызова;
        // NDK копирует структуру колбэков; контексты переживут сессию (state — до конца бэкенда,
        // capture — до закрытия устройства); `raw` — валидное выходное место.
        let status = unsafe {
            match &session_request {
                Some(request) => ndk::ACameraDevice_createCaptureSessionWithSessionParameters(
                    device.as_ptr(),
                    outputs.container.as_ptr(),
                    request.as_ptr(),
                    &state_callbacks,
                    &mut raw,
                ),
                None => ndk::ACameraDevice_createCaptureSession(
                    device.as_ptr(),
                    outputs.container.as_ptr(),
                    &state_callbacks,
                    &mut raw,
                ),
            }
        };
        let Some(raw) = NonNull::new(raw).filter(|_| status == tags::STATUS_OK) else {
            return Err(configure_failure("ACameraDevice_createCaptureSession", status));
        };
        device.adopt_capture_context(Arc::clone(&capture_context));
        let session = Self {
            handle: SessionHandle {
                raw,
                state_context: Arc::clone(&state_context),
                capture_context,
            },
            target,
            reader,
            generation: spec.generation,
            stream: spec.stream.clone(),
            mechanism: spec.mechanism,
        };
        Ok((session, state_context))
    }

    pub(super) fn generation(&self) -> SessionGeneration {
        self.generation
    }

    pub(super) fn stream(&self) -> &ResolvedStream {
        &self.stream
    }

    pub(super) fn mechanism(&self) -> FrameRateMechanism {
        self.mechanism
    }

    /// Ставит (или заменяет) повторяющийся запрос. Запрос собирается заново целиком:
    /// каждое значение приходит из плана, остатков прошлого запроса в нём нет.
    pub(super) fn set_repeating_request(
        &mut self,
        device: &OpenedDevice,
        plan: &RequestPlan,
        vendor: &VendorCapabilities,
    ) -> Result<(), RequestFailure> {
        let mut request = build_request(device, plan, vendor).map_err(RequestFailure::Backend)?;
        request.add_target(&self.target).map_err(RequestFailure::Backend)?;

        let mut callbacks = ndk::ACameraCaptureSession_captureCallbacksV2 {
            context: self.handle.capture_context.as_callback_context(),
            onCaptureStarted: Some(capture_on_started),
            onCaptureProgressed: None,
            onCaptureCompleted: Some(capture_on_completed),
            onCaptureFailed: Some(capture_on_failed),
            onCaptureSequenceCompleted: None,
            onCaptureSequenceAborted: None,
            onCaptureBufferLost: None,
        };
        let mut request_pointer = request.as_ptr();
        // SAFETY: сессия, запрос и цель живы; NDK копирует структуру колбэков и запрос; контекст
        // колбэков живёт до закрытия устройства; идентификатор последовательности не нужен (null).
        let status = unsafe {
            ndk::ACameraCaptureSession_setRepeatingRequestV2(
                self.handle.raw.as_ptr(),
                &mut callbacks,
                1,
                &mut request_pointer,
                ptr::null_mut(),
            )
        };
        if status == tags::STATUS_OK {
            Ok(())
        } else {
            Err(request_failure("ACameraCaptureSession_setRepeatingRequestV2", status))
        }
    }

    pub(super) fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        // SAFETY: сессия жива и принадлежит нам.
        let status = unsafe { ndk::ACameraCaptureSession_stopRepeating(self.handle.raw.as_ptr()) };
        if status == tags::STATUS_OK {
            Ok(())
        } else {
            Err(request_failure("ACameraCaptureSession_stopRepeating", status))
        }
    }

    /// Закрывает сессию и останавливает приём кадров, но reader не отбрасывает, а отдаёт вызывающему:
    /// отложить его (а потом удалить) можно только после того, как устройство отпустило окно
    /// (следующая сессия или закрытие устройства).
    ///
    /// Порядок: сессия закрыта (камера больше не пишет в окно по этой сессии) → `stop_offering`
    /// (поток отпускает свои кадры, ворота слушателя закрыты).
    pub(super) fn into_parked_reader(self) -> RawReader {
        let Self {
            handle, target, reader, ..
        } = self;
        drop(handle);
        drop(target);
        reader.stop_offering();
        reader
    }
}

/// Собирает запрос из плана: создаёт `ACaptureRequest` и применяет записи.
fn build_request(
    device: &OpenedDevice,
    plan: &RequestPlan,
    vendor: &VendorCapabilities,
) -> Result<CaptureRequestHandle, BackendFailure> {
    let mut request = CaptureRequestHandle::create(device, plan.template)?;
    apply_entries(&plan.entries, vendor, &mut request).map_err(|error| BackendFailure {
        operation: "apply_request_entries",
        code: error.platform_status(),
        detail: error.to_string(),
    })?;
    Ok(request)
}
