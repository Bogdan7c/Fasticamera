//! Fake-бэкенд камеры с программируемыми отказами — для тестов движка, приложения и сценариев.
//!
//! [`FakeCameraBackend`] живёт на потоке камеры и реализует [`CameraBackend`]; тест держит
//! разделяемый [`FakeBackendControl`] и через него:
//! * задаёт каталог и ответы на следующие вызовы (очереди по методам);
//! * программирует вытеснение прямо во время `configure_session`;
//! * шлёт события бэкенда с текущими поколениями (`disconnect_current_device`,
//!   `fail_current_device`, `complete_frames`);
//! * читает журнал вызовов и текущее состояние («устройство открыто?»).
//!
//! Автоматика 3A (сессия 08) — в [`three_a`]: журнал триггеров и серий, сценарий состояний AF,
//! результаты триггера и кадров серии, отказы отправки.
//!
//! Fake сам соблюдает контракт трейта: `close_session` и `close_device` идемпотентны,
//! `open_device` при открытом устройстве сначала закрывает старое.

use std::collections::VecDeque;
use std::panic::resume_unwind;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use camera_contract::three_a::RequestOrigin;

use camera_contract::capabilities::CameraDescriptor;
use camera_contract::control::CaptureIntent;
use camera_contract::fixtures::oneplus11_visible_cameras;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, FrameRateMechanism, ResolvedStream};
use camera_contract::three_a::{AfState, BurstId, ExposureBracket, PointRequestId};

use crate::backend::{
    BackendEvent, BackendEventSink, BackendFactory, BackendFailure, CameraBackend, ConfigureFailure, DeviceErrorKind,
    DeviceGeneration, OpenFailure, RequestFailure, SessionGeneration,
};
use crate::backend_standby::{StandbySlotId, StandbySupport};
use crate::request_controls::{AfTrigger, RequestControls};

/// Вызов бэкенда в журнале (с аргументами и поколениями).
#[derive(Debug, Clone, PartialEq)]
pub enum BackendCall {
    LoadCatalog,
    OpenDevice {
        camera: CameraId,
        generation: DeviceGeneration,
    },
    ConfigureSession {
        stream: ResolvedStream,
        generation: SessionGeneration,
    },
    SetRepeatingRequest {
        intent: CaptureIntent,
        controls: RequestControls,
    },
    SubmitFocusTrigger {
        intent: CaptureIntent,
        controls: RequestControls,
        trigger: AfTrigger,
        request: PointRequestId,
    },
    SubmitBracket {
        intent: CaptureIntent,
        base: RequestControls,
        bracket: ExposureBracket,
        burst: BurstId,
    },
    /// Разовый запрос со вспышкой (14d).
    SubmitFlashStill {
        intent: CaptureIntent,
        controls: RequestControls,
        shot: camera_contract::flash::FlashShotId,
    },
    StopRepeating,
    CloseSession,
    CloseDevice,
    OpenStandbyDevice {
        slot: StandbySlotId,
        camera: CameraId,
        generation: DeviceGeneration,
    },
    ConfigureStandbySession {
        slot: StandbySlotId,
        stream: ResolvedStream,
        generation: SessionGeneration,
    },
    SetStandbyRepeatingRequest {
        slot: StandbySlotId,
        intent: CaptureIntent,
        controls: RequestControls,
    },
    CloseStandby {
        slot: StandbySlotId,
    },
    SwapStandbyWithPrimary {
        slot: StandbySlotId,
    },
}

/// Метод бэкенда — для программирования паники.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMethod {
    LoadCatalog,
    OpenDevice,
    ConfigureSession,
    SetRepeatingRequest,
    SubmitFocusTrigger,
    SubmitBracket,
    SubmitFlashStill,
    StopRepeating,
    CloseSession,
    CloseDevice,
    OpenStandbyDevice,
    ConfigureStandbySession,
    SetStandbyRepeatingRequest,
    CloseStandby,
    SwapStandby,
}

/// Итог попытки послать событие через fake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeEventDelivery {
    Sent,
    /// Нет устройства/сессии, чьё поколение поставить на событие, или бэкенд ещё не создан.
    NothingToAttachTo,
}

/// Что fake сделает при следующем `configure_session`.
#[derive(Debug, Clone, PartialEq)]
enum ConfigureStep {
    Fail(ConfigureFailure),
    /// Вытеснение посреди конфигурации: событие `DeviceDisconnected` через sink **и**
    /// `Err(DeviceDisconnected)` из вызова — как на телефоне.
    DisconnectDuring,
}

struct FakeState {
    cameras: Vec<CameraDescriptor>,
    catalog_failure: Option<BackendFailure>,
    open_script: VecDeque<Result<(), OpenFailure>>,
    configure_script: VecDeque<ConfigureStep>,
    request_script: VecDeque<Result<(), RequestFailure>>,
    stop_script: VecDeque<Result<(), RequestFailure>>,
    panic_script: VecDeque<BackendMethod>,
    calls: Vec<BackendCall>,
    device: Option<(CameraId, DeviceGeneration)>,
    session: Option<SessionGeneration>,
    repeating: Option<(CaptureIntent, RequestControls)>,
    trigger_script: VecDeque<Result<(), RequestFailure>>,
    bracket_script: VecDeque<Result<(), RequestFailure>>,
    /// Состояния AF для следующих результатов (по одному на результат).
    af_script: VecDeque<AfState>,
    submitted_brackets: Vec<(BurstId, ExposureBracket)>,
    /// Сценарий отказов `submit_flash_still` (14d; пустая очередь — успех).
    flash_script: VecDeque<Result<(), RequestFailure>>,
    /// Принятые разовые запросы со вспышкой: из них строятся их результаты.
    submitted_flash_stills: Vec<(camera_contract::flash::FlashShotId, RequestControls)>,
    sink: Option<BackendEventSink>,
    standby: standby::FakeStandbyState,
    next_frame_number: i64,
    next_timestamp_nanos: i64,
}

mod flash;
mod standby;
mod three_a;

/// Разделяемая ручка управления fake-бэкендом (клонируется через `Arc`).
pub struct FakeBackendControl {
    state: Mutex<FakeState>,
}

impl FakeBackendControl {
    /// Fake с каталогом OnePlus 11 по умолчанию.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeState {
                cameras: oneplus11_visible_cameras(),
                catalog_failure: None,
                open_script: VecDeque::new(),
                configure_script: VecDeque::new(),
                request_script: VecDeque::new(),
                stop_script: VecDeque::new(),
                panic_script: VecDeque::new(),
                calls: Vec::new(),
                device: None,
                session: None,
                repeating: None,
                trigger_script: VecDeque::new(),
                bracket_script: VecDeque::new(),
                af_script: VecDeque::new(),
                submitted_brackets: Vec::new(),
                flash_script: VecDeque::new(),
                submitted_flash_stills: Vec::new(),
                sink: None,
                standby: standby::FakeStandbyState::new(),
                next_frame_number: 0,
                next_timestamp_nanos: 1_000_000_000,
            }),
        })
    }

    /// Фабрика для [`crate::CameraRuntime::spawn`]: создаёт [`FakeCameraBackend`] на потоке камеры.
    pub fn factory(self: &Arc<Self>) -> BackendFactory {
        let control = Arc::clone(self);
        Box::new(move |sink| {
            control.lock().sink = Some(sink);
            Ok(Box::new(FakeCameraBackend {
                control: Arc::clone(&control),
            }) as Box<dyn CameraBackend>)
        })
    }

    /// Сам бэкенд без runtime (для прямых тестов движка); `sink` — куда fake шлёт события.
    pub fn backend(self: &Arc<Self>, sink: BackendEventSink) -> Box<dyn CameraBackend> {
        self.lock().sink = Some(sink);
        Box::new(FakeCameraBackend {
            control: Arc::clone(self),
        })
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- Программирование ответов ----

    pub fn set_cameras(&self, cameras: Vec<CameraDescriptor>) {
        self.lock().cameras = cameras;
    }

    /// `load_catalog` будет возвращать эту ошибку, пока её не снимут (`None`).
    pub fn set_catalog_failure(&self, failure: Option<BackendFailure>) {
        self.lock().catalog_failure = failure;
    }

    /// Следующий `open_device` вернёт этот итог (очередь; пустая — успех).
    pub fn push_open_result(&self, result: Result<(), OpenFailure>) {
        self.lock().open_script.push_back(result);
    }

    pub fn push_configure_failure(&self, failure: ConfigureFailure) {
        self.lock().configure_script.push_back(ConfigureStep::Fail(failure));
    }

    /// Следующий `configure_session`: вытеснение посреди вызова (событие и `Err`).
    pub fn push_disconnect_during_configure(&self) {
        self.lock().configure_script.push_back(ConfigureStep::DisconnectDuring);
    }

    /// Следующий `set_repeating_request` вернёт этот итог.
    pub fn push_request_result(&self, result: Result<(), RequestFailure>) {
        self.lock().request_script.push_back(result);
    }

    pub fn push_stop_result(&self, result: Result<(), RequestFailure>) {
        self.lock().stop_script.push_back(result);
    }

    /// Следующий вызов этого метода запаникует (для проверки перехвата паники движком).
    pub fn push_panic_on(&self, method: BackendMethod) {
        self.lock().panic_script.push_back(method);
    }

    // ---- События ----

    /// Любое событие как есть (например, с чужим поколением для проверки stale).
    pub fn send_event(&self, event: BackendEvent) -> FakeEventDelivery {
        let sink = self.lock().sink.clone();
        match sink {
            Some(sink) => {
                sink.deliver(event);
                FakeEventDelivery::Sent
            }
            None => FakeEventDelivery::NothingToAttachTo,
        }
    }

    pub fn disconnect_current_device(&self) -> FakeEventDelivery {
        match self.current_device_generation() {
            Some(device) => self.send_event(BackendEvent::DeviceDisconnected { device }),
            None => FakeEventDelivery::NothingToAttachTo,
        }
    }

    pub fn fail_current_device(&self, error: DeviceErrorKind) -> FakeEventDelivery {
        match self.current_device_generation() {
            Some(device) => self.send_event(BackendEvent::DeviceError { device, error }),
            None => FakeEventDelivery::NothingToAttachTo,
        }
    }

    pub fn close_current_session_from_platform(&self) -> FakeEventDelivery {
        match self.current_session_generation() {
            Some(session) => self.send_event(BackendEvent::SessionClosed { session }),
            None => FakeEventDelivery::NothingToAttachTo,
        }
    }

    /// `count` результатов повторяющегося запроса текущей сессии с метками через `interval`
    /// друг от друга (метки продолжаются с места, где остановились прошлые вызовы).
    /// Экспозиция в результате — та, что запрошена планом `Sensor`, иначе 8 мс и ISO 100.
    pub fn complete_frames(&self, count: u32, interval: Duration) -> FakeEventDelivery {
        let Some(session) = self.current_session_generation() else {
            return FakeEventDelivery::NothingToAttachTo;
        };
        for _ in 0..count {
            let result = self.next_result(RequestOrigin::Repeating, interval);
            self.send_event(BackendEvent::CaptureCompleted { session, result });
        }
        FakeEventDelivery::Sent
    }

    // ---- Наблюдение ----

    /// Копия журнала вызовов.
    pub fn calls(&self) -> Vec<BackendCall> {
        self.lock().calls.clone()
    }

    /// Забрать журнал и очистить его (удобно сравнивать «что было с прошлой проверки»).
    pub fn take_calls(&self) -> Vec<BackendCall> {
        std::mem::take(&mut self.lock().calls)
    }

    pub fn is_device_open(&self) -> bool {
        self.lock().device.is_some()
    }

    pub fn open_camera(&self) -> Option<CameraId> {
        self.lock().device.as_ref().map(|(camera, _)| camera.clone())
    }

    pub fn is_session_configured(&self) -> bool {
        self.lock().session.is_some()
    }

    pub fn is_repeating(&self) -> bool {
        self.lock().repeating.is_some()
    }

    pub fn current_device_generation(&self) -> Option<DeviceGeneration> {
        self.lock().device.as_ref().map(|(_, generation)| *generation)
    }

    pub fn current_session_generation(&self) -> Option<SessionGeneration> {
        self.lock().session
    }

    /// Параметры последнего активного повторяющегося запроса.
    pub fn repeating_request(&self) -> Option<(CaptureIntent, RequestControls)> {
        self.lock().repeating
    }
}

/// Бэкенд-заглушка: делегирует всё в [`FakeBackendControl`].
pub struct FakeCameraBackend {
    control: Arc<FakeBackendControl>,
}

impl FakeCameraBackend {
    /// Записать вызов в журнал; запаниковать, если тест так запрограммировал.
    fn enter(&self, method: BackendMethod, call: BackendCall) {
        let should_panic = {
            let mut state = self.control.lock();
            state.calls.push(call);
            if state.panic_script.front() == Some(&method) {
                state.panic_script.pop_front();
                true
            } else {
                false
            }
        };
        if should_panic {
            // `resume_unwind` — паника без вывода в stderr и без lint `clippy::panic`.
            resume_unwind(Box::new(format!(
                "fake-бэкенд: запрограммированная паника в {method:?}"
            )));
        }
    }
}

impl CameraBackend for FakeCameraBackend {
    fn load_catalog(&mut self) -> Result<Vec<CameraDescriptor>, BackendFailure> {
        self.enter(BackendMethod::LoadCatalog, BackendCall::LoadCatalog);
        let state = self.control.lock();
        match &state.catalog_failure {
            Some(failure) => Err(failure.clone()),
            None => Ok(state.cameras.clone()),
        }
    }

    fn open_device(&mut self, camera: &CameraId, generation: DeviceGeneration) -> Result<(), OpenFailure> {
        self.enter(
            BackendMethod::OpenDevice,
            BackendCall::OpenDevice {
                camera: camera.clone(),
                generation,
            },
        );
        let mut state = self.control.lock();
        // Контракт: открытое устройство сначала закрывается.
        state.device = None;
        state.session = None;
        state.repeating = None;
        match state.open_script.pop_front() {
            Some(Err(failure)) => Err(failure),
            Some(Ok(())) | None => {
                state.device = Some((camera.clone(), generation));
                Ok(())
            }
        }
    }

    fn configure_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        self.enter(
            BackendMethod::ConfigureSession,
            BackendCall::ConfigureSession {
                stream: stream.clone(),
                generation,
            },
        );
        let mut state = self.control.lock();
        state.session = None;
        state.repeating = None;
        let Some((_, device)) = state.device.clone() else {
            return Err(ConfigureFailure::Backend(BackendFailure {
                operation: "fake.configure_session",
                code: -1,
                detail: "устройство не открыто".to_owned(),
            }));
        };
        match state.configure_script.pop_front() {
            Some(ConfigureStep::Fail(failure)) => Err(failure),
            Some(ConfigureStep::DisconnectDuring) => {
                let sink = state.sink.clone();
                drop(state);
                if let Some(sink) = sink {
                    sink.deliver(BackendEvent::DeviceDisconnected { device });
                }
                Err(ConfigureFailure::DeviceDisconnected)
            }
            None => {
                state.session = Some(generation);
                Ok(ConfiguredStream {
                    stream: stream.clone(),
                    frame_rate_mechanism: FrameRateMechanism::HalDefault,
                })
            }
        }
    }

    fn set_repeating_request(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure> {
        self.enter(
            BackendMethod::SetRepeatingRequest,
            BackendCall::SetRepeatingRequest {
                intent,
                controls: *controls,
            },
        );
        let mut state = self.control.lock();
        if let Some(scripted) = state.request_script.pop_front() {
            return scripted;
        }
        if state.session.is_none() {
            return Err(RequestFailure::NoSession);
        }
        state.repeating = Some((intent, *controls));
        Ok(())
    }

    fn submit_focus_trigger(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        trigger: AfTrigger,
        request: PointRequestId,
    ) -> Result<(), RequestFailure> {
        self.enter(
            BackendMethod::SubmitFocusTrigger,
            BackendCall::SubmitFocusTrigger {
                intent,
                controls: *controls,
                trigger,
                request,
            },
        );
        let mut state = self.control.lock();
        match state.trigger_script.pop_front() {
            Some(scripted) => scripted,
            None if state.session.is_none() => Err(RequestFailure::NoSession),
            None => Ok(()),
        }
    }

    fn submit_flash_still(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        shot: camera_contract::flash::FlashShotId,
    ) -> Result<(), RequestFailure> {
        self.enter(
            BackendMethod::SubmitFlashStill,
            BackendCall::SubmitFlashStill {
                intent,
                controls: *controls,
                shot,
            },
        );
        self.control.accept_flash_still(controls, shot)
    }

    fn submit_bracket(
        &mut self,
        intent: CaptureIntent,
        base: &RequestControls,
        bracket: &ExposureBracket,
        burst: BurstId,
    ) -> Result<(), RequestFailure> {
        self.enter(
            BackendMethod::SubmitBracket,
            BackendCall::SubmitBracket {
                intent,
                base: *base,
                bracket: bracket.clone(),
                burst,
            },
        );
        let mut state = self.control.lock();
        let outcome = match state.bracket_script.pop_front() {
            Some(scripted) => scripted,
            None if state.session.is_none() => Err(RequestFailure::NoSession),
            None => Ok(()),
        };
        if outcome.is_ok() {
            state.submitted_brackets.push((burst, bracket.clone()));
        }
        outcome
    }

    fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        self.enter(BackendMethod::StopRepeating, BackendCall::StopRepeating);
        let mut state = self.control.lock();
        if let Some(scripted) = state.stop_script.pop_front() {
            return scripted;
        }
        state.repeating = None;
        Ok(())
    }

    fn close_session(&mut self) {
        self.enter(BackendMethod::CloseSession, BackendCall::CloseSession);
        let mut state = self.control.lock();
        state.session = None;
        state.repeating = None;
    }

    fn close_device(&mut self) {
        self.enter(BackendMethod::CloseDevice, BackendCall::CloseDevice);
        let mut state = self.control.lock();
        state.device = None;
        state.session = None;
        state.repeating = None;
    }

    fn standby(&mut self) -> StandbySupport<'_> {
        standby::standby_support(self)
    }
}
