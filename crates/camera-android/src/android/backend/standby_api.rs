//! `StandbyCamera` для `AndroidCameraBackend` (13b): тёплые камеры в ячейках и обмен ролей.
//!
//! # Модель
//! Основная линия бэкенда (`device`/`session`/`parked_outputs`) и каждая ячейка (`StandbyLine`)
//! устроены одинаково. Тёплая — полноценная сессия с RAW-reader-ом **полного** пула и пустым
//! вспомогательным потоком 3A, сконфигурированная и спланированная тем же кодом, что и основная
//! (`session_build`). Отличается только роль сессии: её результаты уходят потоку кадров, а не движку.
//!
//! # Обмен ролей
//! Порядок шагов и его обоснование — в `standby_swap`; здесь [`BackendSwapPorts`] исполняет их над
//! настоящими ресурсами. Основная камера при закрытии (`close_device`) ячеек не трогает: тёплые
//! закрывает движок явно (`close_standby`) или остановка бэкенда.

use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, ResolvedStream};
use camera_core::backend::{
    BackendFailure, ConfigureFailure, DeviceGeneration, OpenFailure, RequestFailure, SessionGeneration,
};
use camera_core::backend_standby::{StandbyCamera, StandbySlotId, StandbySupport, StandbySwapFailure};
use camera_core::request_controls::RequestControls;
use raw_frame_stream::{RawFrameProducer, StandbyPromotion, StreamEpoch};

use super::session_build::{RequestEnv, SessionEnv, apply_repeating_request, build_session};
use super::{AndroidCameraBackend, LOG_TARGET};
use crate::android::device::OpenedDevice;
use crate::android::outputs::ParkedOutputs;
use crate::android::session::ActiveSession;
use crate::android::standby_slot::StandbyLine;
use crate::result_routing::CaptureRole;
use crate::standby_swap::{SwapPorts, swap_with_primary};

/// Отказ из-за номера ячейки за пределами ёмкости (движок выдаёт номера в пределах `capacity()`).
fn slot_out_of_range(operation: &'static str, slot: StandbySlotId, capacity: usize) -> BackendFailure {
    BackendFailure {
        operation,
        code: 0,
        detail: format!("ячейка {} за пределами ёмкости {capacity}", slot.index()),
    }
}

impl AndroidCameraBackend {
    /// Поддержка тёплых: есть, если конфиг оставил хотя бы одну ячейку.
    pub(super) fn standby_support(&mut self) -> StandbySupport<'_> {
        if self.standby.capacity() == 0 {
            StandbySupport::Unsupported
        } else {
            StandbySupport::Available(self)
        }
    }

    /// Закрывает все тёплые (остановка бэкенда).
    pub(super) fn close_all_standby(&mut self) {
        self.standby.close_all();
    }
}

impl StandbyCamera for AndroidCameraBackend {
    fn capacity(&self) -> usize {
        self.standby.capacity()
    }

    fn open_standby_device(
        &mut self,
        slot: StandbySlotId,
        camera: &CameraId,
        generation: DeviceGeneration,
    ) -> Result<(), OpenFailure> {
        self.reap_retired_readers();
        let capacity = self.standby.capacity();
        let Some(line) = self.standby.line_mut(slot) else {
            return Err(OpenFailure::Backend(slot_out_of_range(
                "open_standby_device",
                slot,
                capacity,
            )));
        };
        // Контракт: ячейка открывается «с нуля»; прежнее содержимое закрывается.
        line.close();
        match OpenedDevice::open(
            &self.manager,
            camera,
            generation,
            self.events.clone(),
            std::sync::Arc::clone(&self.diagnostics),
        ) {
            Ok(device) => {
                line.device = Some(device);
                Ok(())
            }
            Err(failure) => {
                self.diagnostics.standby().record_open_failure(slot);
                if let OpenFailure::Backend(backend) = &failure {
                    self.note_failure(backend);
                }
                tracing::warn!(target: LOG_TARGET, slot = slot.index(), camera = %camera, %failure, "тёплая камера не открылась");
                Err(failure)
            }
        }
    }

    fn configure_standby_session(
        &mut self,
        slot: StandbySlotId,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        self.reap_retired_readers();
        let capacity = self.standby.capacity();
        let Some(line) = self.standby.line_mut(slot) else {
            return Err(ConfigureFailure::Backend(slot_out_of_range(
                "configure_standby_session",
                slot,
                capacity,
            )));
        };
        let Some(device) = line.device.as_mut() else {
            return Err(ConfigureFailure::DeviceDisconnected);
        };
        // Прежнюю сессию ячейки закрываем до новой (как у основной): её читатели паркуются.
        if let Some(previous) = line.session.take() {
            line.parked_outputs = Some(previous.into_parked_outputs());
        }
        let env = SessionEnv {
            config: &self.config,
            lens_shading: &self.lens_shading,
            catalog: &self.catalog,
            events: &self.events,
            diagnostics: &self.diagnostics,
            producer: &self.producer,
            cpu_probe: &self.cpu_probe,
            retired_readers: &self.retired_readers,
        };
        // `line.device` заимствован выше; сборке нужен только он, а сессия кладётся после.
        match build_session(&env, device, stream, generation, CaptureRole::Standby(slot)) {
            Ok(built) => {
                self.diagnostics.record_retained_session_context();
                self.diagnostics.standby().record_session_started(slot);
                self.retained_state_contexts.push(built.state_context);
                line.session = Some(built.session);
                Ok(built.configured)
            }
            Err(failure) => {
                self.diagnostics.standby().record_configure_failure(slot);
                self.diagnostics
                    .record_failure("configure_standby_session", 0, &failure.to_string());
                Err(failure)
            }
        }
    }

    fn set_standby_repeating_request(
        &mut self,
        slot: StandbySlotId,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure> {
        self.reap_retired_readers();
        let Some(line) = self.standby.line_mut(slot) else {
            return Err(RequestFailure::NoSession);
        };
        let (Some(device), Some(session)) = (line.device.as_ref(), line.session.as_mut()) else {
            return Err(RequestFailure::NoSession);
        };
        let env = RequestEnv {
            config: &self.config,
            catalog: &self.catalog,
            diagnostics: &self.diagnostics,
        };
        apply_repeating_request(&env, device, session, intent, controls).inspect_err(|_| {
            self.diagnostics.standby().record_configure_failure(slot);
        })
    }

    fn close_standby(&mut self, slot: StandbySlotId) {
        self.reap_retired_readers();
        if let Some(line) = self.standby.line_mut(slot) {
            if line.is_open() {
                tracing::debug!(target: LOG_TARGET, slot = slot.index(), "закрытие тёплой камеры");
            }
            line.close();
        }
        self.reap_retired_readers();
    }

    fn swap_standby_with_primary(&mut self, slot: StandbySlotId) -> Result<(), StandbySwapFailure> {
        self.reap_retired_readers();
        let Some(line) = self.standby.line_mut(slot) else {
            self.diagnostics.standby().record_swap_refusal(slot);
            return Err(StandbySwapFailure::StandbyNotStreaming);
        };
        let mut ports = BackendSwapPorts {
            primary_device: &mut self.device,
            primary_session: &mut self.session,
            primary_parked: &mut self.parked_outputs,
            line,
            producer: &self.producer,
        };
        let outcome = swap_with_primary(&mut ports, slot);
        match &outcome {
            Ok(()) => self.diagnostics.standby().record_swap(slot),
            Err(_) => self.diagnostics.standby().record_swap_refusal(slot),
        }
        outcome
    }
}

/// Порты обмена над настоящими ресурсами бэкенда: основная линия и одна ячейка.
struct BackendSwapPorts<'a> {
    primary_device: &'a mut Option<OpenedDevice>,
    primary_session: &'a mut Option<ActiveSession>,
    primary_parked: &'a mut Option<ParkedOutputs>,
    line: &'a mut StandbyLine,
    producer: &'a RawFrameProducer,
}

impl SwapPorts for BackendSwapPorts<'_> {
    fn standby_epoch(&self, _slot: StandbySlotId) -> Option<StreamEpoch> {
        self.line
            .session
            .as_ref()
            .filter(|session| session.is_streaming())
            .map(ActiveSession::epoch)
    }

    fn primary_is_streaming(&self) -> bool {
        self.primary_session.as_ref().is_some_and(ActiveSession::is_streaming)
    }

    fn promote_in_frame_stream(&mut self, standby: StreamEpoch) -> StandbyPromotion {
        self.producer.promote_standby(standby)
    }

    fn flip_result_roles(&mut self, slot: StandbySlotId) {
        // Проверки обмена гарантируют обе сессии; `if let` — защита без паники.
        if let Some(session) = self.line.session.as_ref() {
            session.result_routing().promote_to_primary();
        }
        if let Some(session) = self.primary_session.as_ref() {
            session.result_routing().demote_to_standby(slot);
        }
    }

    fn exchange_resources(&mut self, _slot: StandbySlotId) {
        std::mem::swap(self.primary_device, &mut self.line.device);
        std::mem::swap(self.primary_session, &mut self.line.session);
        std::mem::swap(self.primary_parked, &mut self.line.parked_outputs);
    }
}
