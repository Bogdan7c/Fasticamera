//! Тёплые камеры fake-бэкенда (13b): ячейки по номеру, журнал вызовов, сценарии отказов и
//! обмен ролей с той же семантикой, что у настоящего бэкенда.
//!
//! * `set_standby_capacity(None)` — бэкенд тёплых не поддерживает (`StandbySupport::Unsupported`);
//! * результатов кадров тёплой fake **не шлёт** (контракт): готовность тёплой —
//!   [`FakeBackendControl::complete_standby_frames`] (одно `StandbyStreaming` на сессию);
//! * после обмена результаты новой основной идут обычным [`FakeBackendControl::complete_frames`],
//!   а запоздавшие результаты бывшей основной тест досылает
//!   [`FakeBackendControl::complete_frames_of_session`].

use std::collections::VecDeque;
use std::time::Duration;

use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, FrameRateMechanism, ResolvedStream};
use camera_contract::three_a::RequestOrigin;

use super::{BackendCall, BackendMethod, FakeBackendControl, FakeCameraBackend, FakeEventDelivery};
use crate::backend::{
    BackendEvent, BackendFailure, CameraBackend, ConfigureFailure, DeviceGeneration, OpenFailure, RequestFailure,
    SessionGeneration,
};
use crate::backend_standby::{StandbyCamera, StandbySlotId, StandbySupport, StandbySwapFailure};
use crate::request_controls::RequestControls;

/// Одна ячейка fake: устройство, сессия и запрос тёплой.
struct FakeSlot {
    device: (CameraId, DeviceGeneration),
    session: Option<SessionGeneration>,
    repeating: Option<(CaptureIntent, RequestControls)>,
    /// `StandbyStreaming` для этой сессии уже отправлено.
    streaming_announced: bool,
}

/// Состояние тёплых внутри `FakeState`.
pub(super) struct FakeStandbyState {
    /// `None` — тёплые не поддерживаются.
    capacity: Option<usize>,
    slots: Vec<Option<FakeSlot>>,
    open_script: VecDeque<Result<(), OpenFailure>>,
    configure_script: VecDeque<Result<(), ConfigureFailure>>,
    request_script: VecDeque<Result<(), RequestFailure>>,
}

impl FakeStandbyState {
    pub(super) fn new() -> Self {
        Self {
            capacity: Some(1),
            slots: Vec::new(),
            open_script: VecDeque::new(),
            configure_script: VecDeque::new(),
            request_script: VecDeque::new(),
        }
    }

    fn slot_mut(&mut self, slot: StandbySlotId) -> &mut Option<FakeSlot> {
        while self.slots.len() <= slot.index() {
            self.slots.push(None);
        }
        &mut self.slots[slot.index()]
    }
}

impl FakeBackendControl {
    /// Ёмкость тёплых; `None` — бэкенд их не поддерживает.
    pub fn set_standby_capacity(&self, capacity: Option<usize>) {
        self.lock().standby.capacity = capacity;
    }

    /// Следующий `open_standby_device` вернёт этот итог.
    pub fn push_standby_open_result(&self, result: Result<(), OpenFailure>) {
        self.lock().standby.open_script.push_back(result);
    }

    /// Следующий `configure_standby_session` вернёт этот итог (`Ok` — сконфигурировать).
    pub fn push_standby_configure_result(&self, result: Result<(), ConfigureFailure>) {
        self.lock().standby.configure_script.push_back(result);
    }

    /// Следующий `set_standby_repeating_request` вернёт этот итог.
    pub fn push_standby_request_result(&self, result: Result<(), RequestFailure>) {
        self.lock().standby.request_script.push_back(result);
    }

    /// Платформа тихо потеряла тёплую (без событий): для проверки отказа обмена ролей.
    pub fn close_standby_silently(&self, slot: StandbySlotId) {
        *self.lock().standby.slot_mut(slot) = None;
    }

    /// Камера, открытая в ячейке.
    pub fn standby_camera(&self, slot: StandbySlotId) -> Option<CameraId> {
        let state = self.lock();
        state
            .standby
            .slots
            .get(slot.index())?
            .as_ref()
            .map(|slot| slot.device.0.clone())
    }

    /// Поколение устройства тёплой в ячейке.
    pub fn standby_device_generation(&self, slot: StandbySlotId) -> Option<DeviceGeneration> {
        let state = self.lock();
        state
            .standby
            .slots
            .get(slot.index())?
            .as_ref()
            .map(|slot| slot.device.1)
    }

    /// Поколение сессии тёплой в ячейке.
    pub fn standby_session_generation(&self, slot: StandbySlotId) -> Option<SessionGeneration> {
        let state = self.lock();
        state.standby.slots.get(slot.index())?.as_ref()?.session
    }

    /// Параметры последнего запроса тёплой.
    pub fn standby_repeating_request(&self, slot: StandbySlotId) -> Option<(CaptureIntent, RequestControls)> {
        let state = self.lock();
        state.standby.slots.get(slot.index())?.as_ref()?.repeating
    }

    /// Тёплая в ячейке отдала кадры: одно `StandbyStreaming` на сессию (повторы ничего не шлют).
    /// Результатов кадров fake не шлёт — так сказано в контракте.
    pub fn complete_standby_frames(&self, slot: StandbySlotId) -> FakeEventDelivery {
        let session = {
            let mut state = self.lock();
            let Some(Some(fake_slot)) = state.standby.slots.get_mut(slot.index()) else {
                return FakeEventDelivery::NothingToAttachTo;
            };
            let Some(session) = fake_slot.session.filter(|_| fake_slot.repeating.is_some()) else {
                return FakeEventDelivery::NothingToAttachTo;
            };
            if std::mem::replace(&mut fake_slot.streaming_announced, true) {
                return FakeEventDelivery::Sent;
            }
            session
        };
        self.send_event(BackendEvent::StandbyStreaming { session })
    }

    /// `count` результатов повторяющегося запроса **заданной** сессии — например, запоздавшие
    /// результаты бывшей основной, которые лежали в ящике, когда произошёл обмен ролей.
    pub fn complete_frames_of_session(
        &self,
        session: SessionGeneration,
        count: u32,
        interval: Duration,
    ) -> FakeEventDelivery {
        for _ in 0..count {
            let result = self.next_result(RequestOrigin::Repeating, interval);
            self.send_event(BackendEvent::CaptureCompleted { session, result });
        }
        FakeEventDelivery::Sent
    }
}

impl FakeCameraBackend {
    fn standby_call(&self, method: BackendMethod, call: BackendCall) {
        self.enter(method, call);
    }
}

fn no_device(operation: &'static str) -> BackendFailure {
    BackendFailure {
        operation,
        code: -1,
        detail: "в ячейке нет открытого устройства".to_owned(),
    }
}

impl StandbyCamera for FakeCameraBackend {
    fn capacity(&self) -> usize {
        self.control.lock().standby.capacity.unwrap_or(0)
    }

    fn open_standby_device(
        &mut self,
        slot: StandbySlotId,
        camera: &CameraId,
        generation: DeviceGeneration,
    ) -> Result<(), OpenFailure> {
        self.standby_call(
            BackendMethod::OpenStandbyDevice,
            BackendCall::OpenStandbyDevice {
                slot,
                camera: camera.clone(),
                generation,
            },
        );
        let mut state = self.control.lock();
        *state.standby.slot_mut(slot) = None;
        match state.standby.open_script.pop_front() {
            Some(Err(failure)) => Err(failure),
            Some(Ok(())) | None => {
                *state.standby.slot_mut(slot) = Some(FakeSlot {
                    device: (camera.clone(), generation),
                    session: None,
                    repeating: None,
                    streaming_announced: false,
                });
                Ok(())
            }
        }
    }

    fn configure_standby_session(
        &mut self,
        slot: StandbySlotId,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        self.standby_call(
            BackendMethod::ConfigureStandbySession,
            BackendCall::ConfigureStandbySession {
                slot,
                stream: stream.clone(),
                generation,
            },
        );
        let mut state = self.control.lock();
        if let Some(Err(failure)) = state.standby.configure_script.pop_front() {
            return Err(failure);
        }
        let Some(fake_slot) = state.standby.slot_mut(slot).as_mut() else {
            return Err(ConfigureFailure::Backend(no_device("fake.configure_standby_session")));
        };
        fake_slot.session = Some(generation);
        fake_slot.repeating = None;
        fake_slot.streaming_announced = false;
        Ok(ConfiguredStream {
            stream: stream.clone(),
            frame_rate_mechanism: FrameRateMechanism::HalDefault,
        })
    }

    fn set_standby_repeating_request(
        &mut self,
        slot: StandbySlotId,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure> {
        self.standby_call(
            BackendMethod::SetStandbyRepeatingRequest,
            BackendCall::SetStandbyRepeatingRequest {
                slot,
                intent,
                controls: *controls,
            },
        );
        let mut state = self.control.lock();
        if let Some(scripted) = state.standby.request_script.pop_front() {
            return scripted;
        }
        match state.standby.slot_mut(slot).as_mut() {
            Some(fake_slot) if fake_slot.session.is_some() => {
                fake_slot.repeating = Some((intent, *controls));
                Ok(())
            }
            _ => Err(RequestFailure::NoSession),
        }
    }

    fn close_standby(&mut self, slot: StandbySlotId) {
        self.standby_call(BackendMethod::CloseStandby, BackendCall::CloseStandby { slot });
        *self.control.lock().standby.slot_mut(slot) = None;
    }

    fn swap_standby_with_primary(&mut self, slot: StandbySlotId) -> Result<(), StandbySwapFailure> {
        self.standby_call(BackendMethod::SwapStandby, BackendCall::SwapStandbyWithPrimary { slot });
        let mut state = self.control.lock();
        if state.session.is_none() || state.repeating.is_none() {
            return Err(StandbySwapFailure::PrimaryNotStreaming);
        }
        let standby_streams = state
            .standby
            .slot_mut(slot)
            .as_ref()
            .is_some_and(|fake_slot| fake_slot.session.is_some() && fake_slot.repeating.is_some());
        if !standby_streams {
            return Err(StandbySwapFailure::StandbyNotStreaming);
        }
        // Роли меняются местами целиком: устройство, сессия и запрос. Прежняя основная
        // остаётся в той же ячейке и стримит (её первое «готова» уже было).
        let Some(mut former_standby) = state.standby.slot_mut(slot).take() else {
            return Err(StandbySwapFailure::StandbyNotStreaming);
        };
        let primary_device = state.device.take();
        let primary_session = state.session.take();
        let primary_repeating = state.repeating.take();
        state.device = Some(former_standby.device.clone());
        state.session = former_standby.session.take();
        state.repeating = former_standby.repeating.take();
        if let Some(device) = primary_device {
            *state.standby.slot_mut(slot) = Some(FakeSlot {
                device,
                session: primary_session,
                repeating: primary_repeating,
                streaming_announced: true,
            });
        }
        Ok(())
    }
}

/// Доступ к тёплым для `CameraBackend::standby` у fake.
pub(super) fn standby_support(backend: &mut FakeCameraBackend) -> StandbySupport<'_> {
    if backend.control.lock().standby.capacity.is_some() {
        StandbySupport::Available(backend)
    } else {
        StandbySupport::Unsupported
    }
}

// Проверка на этапе компиляции: fake — полноценный бэкенд с тёплыми.
const _: fn(&mut FakeCameraBackend) -> &mut dyn CameraBackend = |backend| backend;
