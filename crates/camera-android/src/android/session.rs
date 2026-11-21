//! Сессия съёмки: контейнер выходов, цель запроса, сама `ACameraCaptureSession`,
//! повторяющийся запрос, разовые запросы (триггер фокуса, серия) и вспомогательный поток 3A.
//!
//! # Порядок разрушения
//!
//! Стоп повтора → закрыть сессию → остановить приём кадров reader-а → (снаружи) закрыть устройство →
//! отложить reader (удаляется, когда все кадры вернулись, см. `raw_reader`). Окно
//! `AImageReader` не должно умереть раньше сессии, которая в него пишет. Поэтому поля
//! [`ActiveSession`] объявлены в порядке разрушения (Rust освобождает поля по порядку объявления):
//! сначала `handle` (стоп + закрытие сессии), затем `target`, затем `reader`. А при закрытии отдельно
//! от устройства reader не отбрасывается сразу, а возвращается вызывающему ([`ActiveSession::into_parked_outputs`])
//! — отложить его можно только когда устройство отсоединилось от окна. Вспомогательный поток 3A
//! (решение 62) паркуется вместе с RAW-читателем (`ParkedOutputs`): его окно привязано к устройству так же.
//!
//! # Запросы и их происхождение
//!
//! Каждый запрос (повторяющийся, триггер, кадр серии) собирается одним путём [`build_targeted_request`]
//! с целью на RAW-окно и помечается ключом происхождения (`request_origin`). Ключи разовых запросов
//! регистрируются **до** отправки и снимаются, если отправка не удалась ([`PendingOrigins`]).

use std::ptr::{self, NonNull};
use std::sync::Arc;

use camera_contract::stream::{FrameRateMechanism, ResolvedStream};
use camera_contract::three_a::RequestOrigin;
use camera_core::backend::{BackendEventSink, BackendFailure, ConfigureFailure, RequestFailure, SessionGeneration};
use camera_ndk_sys as ndk;
use raw_frame_stream::RawFrameProducer;

use crate::companion_negotiation::{AttemptError, negotiate_session};
use crate::companion_stream::{CompanionStreamSpec, CompanionStreamStatus, CompanionUnavailableReason};
use crate::diagnostics::BackendDiagnostics;
use crate::request_origin::{RequestKey, RequestOriginTable};
use crate::request_plan::{RequestPlan, apply_entries};
use crate::result_routing::{CaptureRole, ResultRouting};
use crate::status_mapping::{camera_backend_failure, configure_failure, request_failure};
use crate::tags;
use crate::vendor::VendorCapabilities;

use super::callbacks::{
    CaptureContext, CaptureContextSpec, SessionStateContext, capture_on_completed, capture_on_failed,
    capture_on_started, session_on_closed, session_on_idle_transition,
};
use super::companion::CompanionReader;
use super::device::OpenedDevice;
use super::outputs::{OutputContainer, OutputTarget, ParkedOutputs};
use super::raw_reader::RawReader;
use super::request::CaptureRequestHandle;

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
/// Порядок полей = порядок разрушения (см. доку модуля): `handle` → `target` → `reader` → `companion`.
pub(super) struct ActiveSession {
    handle: SessionHandle,
    /// Цель запросов — окно RAW-читателя. Вспомогательный поток целью **не бывает никогда**.
    target: OutputTarget,
    reader: RawReader,
    /// Вспомогательный поток 3A, если он вошёл в сессию (решение 62).
    companion: Option<CompanionReader>,
    generation: SessionGeneration,
    stream: ResolvedStream,
    mechanism: FrameRateMechanism,
    diagnostics: Arc<BackendDiagnostics>,
    /// Повторяющийся запрос стоит (кадры идут): обмен ролей требует этого от обеих сторон, как fake.
    repeating_active: bool,
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
    /// Вспомогательный поток 3A; `None` — выключен настройкой.
    pub companion: Option<CompanionStreamSpec>,
    /// Размер таблицы происхождений разовых запросов.
    pub origin_capacity: usize,
    /// Начальная роль сессии: основная (результаты движку) или тёплая в ячейке (13b).
    pub role: CaptureRole,
}

/// Один запрос для разовой отправки: готовый план и происхождение, которым пометятся его результаты.
pub(super) struct OneShotRequest<'a> {
    pub plan: &'a RequestPlan,
    pub origin: RequestOrigin,
}

impl ActiveSession {
    /// Создаёт сессию на открытом устройстве. На любой ошибке ничего не течёт: приёмник и
    /// вспомогательные объекты освобождаются RAII.
    ///
    /// Вспомогательный поток 3A добавляется в контейнер выходов, если он включён; не вышло — сессия
    /// создаётся заново без него (`negotiate_session`), а диагностика говорит, что автоматика HAL мертва.
    pub(super) fn create(
        device: &mut OpenedDevice,
        reader: RawReader,
        spec: &SessionSpec<'_>,
        event_sink: &BackendEventSink,
        diagnostics: &Arc<BackendDiagnostics>,
    ) -> Result<(Self, Arc<SessionStateContext>), ConfigureFailure> {
        let backend_failure = |failure: BackendFailure| ConfigureFailure::Backend(failure);
        let target = OutputTarget::create(reader.window()).map_err(backend_failure)?;

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
            role: spec.role,
            frame_number_window: spec.frame_number_window,
            origin_capacity: spec.origin_capacity,
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
        let negotiated = negotiate_session(
            spec.companion,
            |companion_spec| CompanionReader::create(companion_spec, Arc::clone(diagnostics)),
            |companion| create_ndk_session(device, &reader, companion, session_request.as_ref(), &state_callbacks),
        )?;
        report_companion(diagnostics, negotiated.status);
        device.adopt_capture_context(Arc::clone(&capture_context));
        let session = Self {
            handle: SessionHandle {
                raw: negotiated.session,
                state_context: Arc::clone(&state_context),
                capture_context,
            },
            target,
            reader,
            companion: negotiated.companion,
            generation: spec.generation,
            stream: spec.stream.clone(),
            mechanism: spec.mechanism,
            diagnostics: Arc::clone(diagnostics),
            repeating_active: false,
        };
        Ok((session, state_context))
    }

    pub(super) fn generation(&self) -> SessionGeneration {
        self.generation
    }

    /// Эпоха потока кадров reader-а этой сессии (по ней обмен ролей находит тёплую в хабе).
    pub(super) fn epoch(&self) -> raw_frame_stream::StreamEpoch {
        self.reader.epoch()
    }

    /// Стоит ли повторяющийся запрос: сессия отдаёт кадры (условие обмена ролей).
    pub(super) fn is_streaming(&self) -> bool {
        self.repeating_active
    }

    /// Роль сессии для колбэков результатов: обмен ролей переключает её одним атомарным store.
    pub(super) fn result_routing(&self) -> &ResultRouting {
        self.handle.capture_context.routing()
    }

    pub(super) fn stream(&self) -> &ResolvedStream {
        &self.stream
    }

    pub(super) fn mechanism(&self) -> FrameRateMechanism {
        self.mechanism
    }

    /// Колбэки результатов: один и тот же набор для повторяющегося запроса и разовых — так
    /// результаты и кадры текут одним путём.
    fn capture_callbacks(&self) -> ndk::ACameraCaptureSession_captureCallbacksV2 {
        ndk::ACameraCaptureSession_captureCallbacksV2 {
            context: self.handle.capture_context.as_callback_context(),
            onCaptureStarted: Some(capture_on_started),
            onCaptureProgressed: None,
            onCaptureCompleted: Some(capture_on_completed),
            onCaptureFailed: Some(capture_on_failed),
            onCaptureSequenceCompleted: None,
            onCaptureSequenceAborted: None,
            onCaptureBufferLost: None,
        }
    }

    /// Ставит (или заменяет) повторяющийся запрос. Запрос собирается заново целиком:
    /// каждое значение приходит из плана, остатков прошлого запроса в нём нет. Метка происхождения —
    /// зарезервированный ключ повторяющегося запроса (в таблице не хранится).
    pub(super) fn set_repeating_request(
        &mut self,
        device: &OpenedDevice,
        plan: &RequestPlan,
        vendor: &VendorCapabilities,
    ) -> Result<(), RequestFailure> {
        let request = build_targeted_request(device, plan, vendor, &self.target, RequestKey::REPEATING)
            .map_err(RequestFailure::Backend)?;

        let mut callbacks = self.capture_callbacks();
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
            self.repeating_active = true;
            Ok(())
        } else {
            Err(request_failure("ACameraCaptureSession_setRepeatingRequestV2", status))
        }
    }

    /// Отправляет разовые запросы одним вызовом `ACameraCaptureSession_captureV2` поверх идущего
    /// повторяющегося. Цель каждого — только RAW-окно; колбэки те же, что у повторяющегося.
    ///
    /// Ключи происхождения регистрируются **до** отправки (колбэк может прийти раньше, чем вернётся
    /// вызов) и снимаются, если отправка или сборка любого запроса не удалась.
    pub(super) fn submit_one_shot(
        &mut self,
        device: &OpenedDevice,
        requests: &[OneShotRequest<'_>],
        vendor: &VendorCapabilities,
    ) -> Result<(), RequestFailure> {
        let count = i32::try_from(requests.len()).map_err(|_| {
            RequestFailure::Backend(camera_backend_failure(
                "ACameraCaptureSession_captureV2",
                tags::STATUS_INVALID_PARAMETER,
            ))
        })?;
        let origins = self.handle.capture_context.origins();
        let mut pending = PendingOrigins::new(origins);
        let mut handles = Vec::with_capacity(requests.len());
        for one_shot in requests {
            let registration = origins.register(one_shot.origin);
            pending.track(registration.key);
            if registration.evicted_oldest {
                self.diagnostics.record_evicted_request_origin();
            }
            let handle = build_targeted_request(device, one_shot.plan, vendor, &self.target, registration.key)
                .map_err(RequestFailure::Backend)?;
            handles.push(handle);
        }
        let mut pointers: Vec<*mut ndk::ACaptureRequest> = handles.iter().map(CaptureRequestHandle::as_ptr).collect();

        let mut callbacks = self.capture_callbacks();
        // SAFETY: сессия, запросы и цель живы до конца вызова; NDK копирует структуру колбэков и
        // запросы; `pointers` хранит ровно `count` валидных указателей; контекст колбэков живёт до
        // закрытия устройства; идентификатор последовательности не нужен (null).
        let status = unsafe {
            ndk::ACameraCaptureSession_captureV2(
                self.handle.raw.as_ptr(),
                &mut callbacks,
                count,
                pointers.as_mut_ptr(),
                ptr::null_mut(),
            )
        };
        if status == tags::STATUS_OK {
            // Запросы приняты: их ключи теперь снимают результаты, а не откат.
            pending.commit();
            Ok(())
        } else {
            Err(request_failure("ACameraCaptureSession_captureV2", status))
        }
    }

    pub(super) fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        // SAFETY: сессия жива и принадлежит нам.
        let status = unsafe { ndk::ACameraCaptureSession_stopRepeating(self.handle.raw.as_ptr()) };
        if status == tags::STATUS_OK {
            self.repeating_active = false;
            Ok(())
        } else {
            Err(request_failure("ACameraCaptureSession_stopRepeating", status))
        }
    }

    /// Закрывает сессию и останавливает приём кадров, но читатели не отбрасывает, а отдаёт вызывающему:
    /// отложить RAW-читатель (а потом удалить) и удалить вспомогательный можно только после того, как
    /// устройство отпустило окна (следующая сессия или закрытие устройства).
    ///
    /// Порядок: сессия закрыта (камера больше не пишет в окна по этой сессии) → `stop_offering`
    /// (поток отпускает свои кадры, ворота слушателя закрыты).
    pub(super) fn into_parked_outputs(self) -> ParkedOutputs {
        let Self {
            handle,
            target,
            reader,
            companion,
            ..
        } = self;
        drop(handle);
        drop(target);
        reader.stop_offering();
        ParkedOutputs::new(reader, companion)
    }
}

/// Одна попытка создать сессию `ACameraCaptureSession` с выходами RAW (+ вспомогательный, если дан).
fn create_ndk_session(
    device: &OpenedDevice,
    reader: &RawReader,
    companion: Option<&CompanionReader>,
    session_request: Option<&CaptureRequestHandle>,
    state_callbacks: &ndk::ACameraCaptureSession_stateCallbacks,
) -> Result<NonNull<ndk::ACameraCaptureSession>, AttemptError> {
    let mut outputs =
        OutputContainer::create().map_err(|failure| AttemptError::Failed(ConfigureFailure::Backend(failure)))?;
    outputs
        .add_window(reader.window())
        .map_err(|failure| AttemptError::Failed(ConfigureFailure::Backend(failure)))?;
    if let Some(companion) = companion {
        outputs
            .add_window(companion.window())
            .map_err(|failure| AttemptError::CompanionOutputRejected { status: failure.code })?;
    }
    let mut raw: *mut ndk::ACameraCaptureSession = ptr::null_mut();
    // SAFETY: устройство открыто; контейнер, параметры (если есть) и колбэки живы на время вызова;
    // NDK копирует структуру колбэков; контексты переживут сессию (state — до конца бэкенда,
    // capture — до закрытия устройства); окна принадлежат читателям, которые живут дольше сессии;
    // `raw` — валидное выходное место.
    let status = unsafe {
        match session_request {
            Some(request) => ndk::ACameraDevice_createCaptureSessionWithSessionParameters(
                device.as_ptr(),
                outputs.as_ptr(),
                request.as_ptr(),
                state_callbacks,
                &mut raw,
            ),
            None => {
                ndk::ACameraDevice_createCaptureSession(device.as_ptr(), outputs.as_ptr(), state_callbacks, &mut raw)
            }
        }
    };
    NonNull::new(raw)
        .filter(|_| status == tags::STATUS_OK)
        .ok_or_else(|| AttemptError::Failed(configure_failure("ACameraDevice_createCaptureSession", status)))
}

/// Состояние вспомогательного потока — в диагностику и журнал; «мертва автоматика» не молчит.
fn report_companion(diagnostics: &BackendDiagnostics, status: CompanionStreamStatus) {
    diagnostics.record_companion_status(status);
    if let CompanionStreamStatus::Unavailable { reason } = status {
        let detail = format!("вспомогательный поток 3A недоступен: {reason:?}; автоматика HAL (AF, AE) не работает");
        tracing::warn!(target: "fasticamera::camera::session", ?reason, "вспомогательный поток 3A недоступен, автоматика HAL мертва");
        let code = match reason {
            CompanionUnavailableReason::ReaderCreationFailed { status }
            | CompanionUnavailableReason::OutputRegistrationFailed { status } => status,
            CompanionUnavailableReason::RejectedByHal => tags::STATUS_STREAM_CONFIGURE_FAIL,
        };
        diagnostics.record_failure("three_a_companion", code, &detail);
    } else if status.hal_three_a_is_dead() {
        tracing::warn!(target: "fasticamera::camera::session", "вспомогательный поток 3A выключен настройкой, автоматика HAL мертва");
    }
}

/// Ключи разовых запросов, зарегистрированные до отправки. Если отправка не удалась (ранний возврат
/// или отказ NDK), `Drop` снимает их все; после успеха [`PendingOrigins::commit`] оставляет ключи
/// до прихода результатов.
struct PendingOrigins<'a> {
    table: &'a RequestOriginTable,
    keys: Vec<RequestKey>,
    committed: bool,
}

impl<'a> PendingOrigins<'a> {
    fn new(table: &'a RequestOriginTable) -> Self {
        Self {
            table,
            keys: Vec::new(),
            committed: false,
        }
    }

    fn track(&mut self, key: RequestKey) {
        self.keys.push(key);
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingOrigins<'_> {
    fn drop(&mut self) {
        if !self.committed {
            for key in &self.keys {
                self.table.unregister(*key);
            }
        }
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

/// Запрос для отправки в сессию: план применён, цель — RAW-окно, метка происхождения поставлена.
fn build_targeted_request(
    device: &OpenedDevice,
    plan: &RequestPlan,
    vendor: &VendorCapabilities,
    target: &OutputTarget,
    key: RequestKey,
) -> Result<CaptureRequestHandle, BackendFailure> {
    let mut request = build_request(device, plan, vendor)?;
    request.add_target(target)?;
    request.tag_origin(key)?;
    Ok(request)
}
