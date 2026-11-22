//! `AndroidCameraBackend` — реализация `CameraBackend` на Camera2 NDK.
//!
//! Все виды запросов (повторяющийся, триггер фокуса, серия, кадр со вспышкой) строятся одним путём
//! `request_plan::plan_request`, поэтому рычаг частоты кадров и вендорские ключи потока есть в каждом.
//!
//! Тонкий слой: решения принимает нейтральная логика (`frame_rate_plan`, `request_plan`,
//! `characteristics`, `status_mapping`), здесь они только применяются через RAII-владельцы NDK.
//! Все вызовы приходят с потока камеры, колбэки NDK — на чужих потоках и только шлют события.

use std::collections::HashMap;
use std::sync::Arc;

use camera_contract::capabilities::CameraDescriptor;
use camera_contract::control::CaptureIntent;
use camera_contract::flash::FlashShotId;
use camera_contract::ids::CameraId;
use camera_contract::raw_format::{MosaicKind, NativeMosaic};
use camera_contract::stream::{ConfiguredStream, ResolvedStream};
use camera_contract::three_a::{BurstId, ExposureBracket, PointRequestId, RequestOrigin};
use camera_core::backend::{
    BackendEventSink, BackendFailure, CameraBackend, ConfigureFailure, DeviceGeneration, OpenFailure, RequestFailure,
    SessionGeneration,
};
use camera_core::backend_standby::StandbySupport;

use camera_core::request_controls::{AfTrigger, RequestControls};
use raw_frame_stream::RawFrameProducer;

use crate::characteristics::{CharacteristicsError, parse_characteristics};
use crate::config::AndroidCameraConfig;
use crate::context_leak::leak_contexts_for_process_lifetime;
use crate::cpu_access_probe::CpuAccessProbe;
use crate::diagnostics::BackendDiagnostics;
use crate::entry::{EntryLookup, EntryValue, MetadataSource};
use crate::flash_still_plan::plan_flash_still;
use crate::request_plan::{PlannedRequest, RequestInputs, plan_bracket, plan_request};
use crate::retired_readers::RetiredReaders;
use crate::sensor_modes::{SensorModeAvailability, SensorModeProblem};
use crate::tags::{self, Tag};
use crate::vendor::{VendorCapabilities, VendorKey};

use super::availability::AvailabilityRegistration;
use super::callbacks::SessionStateContext;
use super::device::OpenedDevice;
use super::manager::CameraManager;
use super::outputs::ParkedOutputs;
use super::raw_reader::RetiredNdkReaders;
use super::session::{ActiveSession, OneShotRequest};

mod session_build;
mod standby_api;

use session_build::{RequestEnv, SessionEnv, apply_repeating_request, build_session};

use super::standby_slot::StandbySlots;
use crate::result_routing::CaptureRole;

const LOG_TARGET: &str = "fasticamera::camera::backend";

/// Камера каталога: описание и разрешённые вендорские теги.
struct CameraRecord {
    descriptor: CameraDescriptor,
    vendor: VendorCapabilities,
    /// Вендорская таблица режимов сенсора: решает, можно ли обещать AE-диапазон выше публичного потолка.
    sensor_modes: SensorModeAvailability,
}

/// Бэкенд камеры на Camera2 NDK.
pub struct AndroidCameraBackend {
    config: AndroidCameraConfig,
    diagnostics: Arc<BackendDiagnostics>,
    events: BackendEventSink,
    /// Вход потока кадров: сюда уходят кадры и результаты запросов (лизинг, сессия 05).
    producer: RawFrameProducer,
    /// Рубильник отладочной пробы CPU-доступа (читается при создании reader-а и на каждый кадр).
    cpu_probe: CpuAccessProbe,
    /// Reader-ы закрытых сессий, ждущие возврата кадров перед удалением.
    retired_readers: Arc<RetiredNdkReaders>,
    catalog: HashMap<CameraId, CameraRecord>,
    // Порядок полей ниже — порядок разрушения как запасной (основной — `Drop`): сессия, приёмник
    // после сессии, затем устройство, затем менеджер.
    session: Option<ActiveSession>,
    /// Читатели закрытой сессии (RAW и вспомогательный поток 3A): живут, пока устройство не
    /// отпустило их окна (см. `session`); RAW-читатель затем отправляется в `retired_readers`.
    parked_outputs: Option<ParkedOutputs>,
    device: Option<OpenedDevice>,
    /// Контексты состояния сессий: не освобождаются до конца бэкенда, а при его разрушении
    /// намеренно утекают (`Drop`, см. доку `callbacks` и `context_leak`).
    retained_state_contexts: Vec<Arc<SessionStateContext>>,
    /// Тёплые камеры (13b): ячейки со своими устройствами и сессиями. Объявлены после основной
    /// линии: при запасном разрушении основная освобождается первой.
    standby: StandbySlots,
    availability: Option<AvailabilityRegistration>,
    manager: CameraManager,
}

impl AndroidCameraBackend {
    /// Создаёт бэкенд на потоке камеры: менеджер Camera2 и колбэки доступности.
    pub fn new(
        config: AndroidCameraConfig,
        events: BackendEventSink,
        diagnostics: Arc<BackendDiagnostics>,
        producer: RawFrameProducer,
        cpu_probe: CpuAccessProbe,
    ) -> Result<Self, BackendFailure> {
        config.validate().map_err(|error| BackendFailure {
            operation: "validate_config",
            code: 0,
            detail: error.to_string(),
        })?;
        let standby_capacity = config.standby_capacity;
        let manager = CameraManager::create()?;
        let availability = match AvailabilityRegistration::register(&manager, events.clone(), Arc::clone(&diagnostics))
        {
            Ok(registration) => Some(registration),
            Err(failure) => {
                // Без колбэков доступности работать можно (вытеснение придёт через onDisconnected),
                // но это нужно видеть: отказ записывается, а не глотается.
                tracing::warn!(target: LOG_TARGET, %failure, "колбэки доступности камер не зарегистрированы");
                diagnostics.record_failure(failure.operation, failure.code, &failure.detail);
                None
            }
        };
        Ok(Self {
            config,
            retired_readers: Arc::new(RetiredReaders::new(Arc::clone(&diagnostics))),
            diagnostics,
            events,
            producer,
            cpu_probe,
            catalog: HashMap::new(),
            session: None,
            parked_outputs: None,
            device: None,
            retained_state_contexts: Vec::new(),
            standby: StandbySlots::new(standby_capacity),
            availability,
            manager,
        })
    }

    /// Читает характеристики одной камеры и собирает запись каталога.
    fn read_camera(&self, id: &str) -> Result<CameraRecord, String> {
        let camera = CameraId::new(id).map_err(|error| error.to_string())?;
        let characteristics = self
            .manager
            .characteristics(id)
            .map_err(|failure| failure.to_string())?;
        let view = characteristics.view();
        let vendor = VendorCapabilities::resolve(&camera, &self.config, &view);
        let native_mosaic = read_native_mosaic(&view, &vendor);
        let sensor_modes = SensorModeAvailability::read(&view, &vendor);
        if let SensorModeAvailability::Unusable(problem) = &sensor_modes {
            note_unusable_sensor_modes(&camera, problem);
        }
        let parsed =
            parse_characteristics(&view, native_mosaic).map_err(|error: CharacteristicsError| error.to_string())?;
        self.diagnostics.record_camera(&camera, vendor.report(), parsed.notes);
        Ok(CameraRecord {
            descriptor: CameraDescriptor {
                id: camera,
                capabilities: parsed.capabilities,
            },
            vendor,
            sensor_modes,
        })
    }

    /// Удаляет reader-ы закрытых сессий, чьи кадры уже вернулись. Зовётся на каждом вызове бэкенда
    /// (поток камеры, без ожидания): так reader уходит вскоре после возврата последнего кадра.
    fn reap_retired_readers(&self) {
        let report = self.retired_readers.reap();
        if report.deleted > 0 {
            tracing::debug!(target: LOG_TARGET, deleted = report.deleted, waiting = report.still_waiting, "reader-ы закрытых сессий удалены");
        }
    }

    /// Отказ платформы: в диагностику и дальше без изменений.
    fn note_failure(&self, failure: &BackendFailure) {
        self.diagnostics
            .record_failure(failure.operation, failure.code, &failure.detail);
    }

    /// Создание новой сессии основной камеры (предыдущая уже закрыта вызывающим).
    fn create_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        let Some(device) = self.device.as_mut() else {
            return Err(ConfigureFailure::DeviceDisconnected);
        };
        let env = SessionEnv {
            config: &self.config,
            catalog: &self.catalog,
            events: &self.events,
            diagnostics: &self.diagnostics,
            producer: &self.producer,
            cpu_probe: &self.cpu_probe,
            retired_readers: &self.retired_readers,
        };
        let built = build_session(&env, device, stream, generation, CaptureRole::Primary)?;
        self.diagnostics.record_retained_session_context();
        self.retained_state_contexts.push(built.state_context);
        self.session = Some(built.session);
        Ok(built.configured)
    }
}

/// Таблица режимов есть, но непригодна: это видно в логе (планировщик на ней откажет в 60 к/с).
fn note_unusable_sensor_modes(camera: &CameraId, problem: &SensorModeProblem) {
    tracing::warn!(target: LOG_TARGET, camera = %camera, %problem, "таблица режимов сенсора непригодна");
}

/// Родная мозаика сенсора по вендорской характеристике `is_qcfa_sensor`; нет тега — неизвестна.
fn read_native_mosaic(source: &dyn MetadataSource, vendor: &VendorCapabilities) -> NativeMosaic {
    let Some(code) = vendor.characteristic_tag(VendorKey::IsQcfaSensor) else {
        return NativeMosaic::Unknown;
    };
    let tag = Tag {
        code,
        name: "vendor:is_qcfa_sensor",
    };
    match source.entry(tag) {
        EntryLookup::Present(EntryValue::Bytes(values)) => match values.first() {
            Some(1) => NativeMosaic::Known(MosaicKind::QuadBayer),
            Some(0) => NativeMosaic::Known(MosaicKind::Bayer),
            _ => NativeMosaic::Unknown,
        },
        _ => NativeMosaic::Unknown,
    }
}

impl CameraBackend for AndroidCameraBackend {
    fn load_catalog(&mut self) -> Result<Vec<CameraDescriptor>, BackendFailure> {
        self.reap_retired_readers();
        self.diagnostics.reset_catalog();
        let ids = self
            .manager
            .camera_ids()
            .inspect_err(|failure| self.note_failure(failure))?;
        self.catalog.clear();
        let mut descriptors = Vec::with_capacity(ids.len());
        for id in ids {
            match self.read_camera(&id) {
                Ok(record) => {
                    descriptors.push(record.descriptor.clone());
                    self.catalog.insert(record.descriptor.id.clone(), record);
                }
                Err(reason) => {
                    // Камеру нельзя описать — её нет в каталоге, но причина видна в диагностике и логе.
                    tracing::warn!(target: LOG_TARGET, camera = %id, %reason, "камера пропущена при чтении каталога");
                    self.diagnostics.record_skipped_camera(&id, reason);
                }
            }
        }
        Ok(descriptors)
    }

    fn open_device(&mut self, camera: &CameraId, generation: DeviceGeneration) -> Result<(), OpenFailure> {
        self.reap_retired_readers();
        // Контракт: открытое устройство сначала закрывается (движок так не делает, но не течём).
        self.close_device();
        match OpenedDevice::open(
            &self.manager,
            camera,
            generation,
            self.events.clone(),
            Arc::clone(&self.diagnostics),
        ) {
            Ok(device) => {
                self.device = Some(device);
                Ok(())
            }
            Err(failure) => {
                if let OpenFailure::Backend(backend) = &failure {
                    self.note_failure(backend);
                }
                tracing::warn!(target: LOG_TARGET, camera = %camera, %failure, "камера не открылась");
                Err(failure)
            }
        }
    }

    fn configure_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        self.reap_retired_readers();
        // Предыдущую сессию закрываем до создания новой (при этом её поток кадров уже остановлен); её
        // reader держим, пока устройство не перешло на новую сессию (отложить reader под работающим
        // потоком нельзя). Прежний припаркованный reader при замене уходит в отсрочку.
        let previous_outputs = self.session.take().map(ActiveSession::into_parked_outputs);
        let outcome = self.create_session(stream, generation);
        if let Some(outputs) = previous_outputs {
            self.parked_outputs = Some(outputs);
        }
        if let Err(failure) = &outcome {
            self.diagnostics
                .record_failure("configure_session", 0, &failure.to_string());
        }
        outcome
    }

    fn set_repeating_request(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure> {
        self.reap_retired_readers();
        let (Some(device), Some(session)) = (self.device.as_ref(), self.session.as_mut()) else {
            return Err(RequestFailure::NoSession);
        };
        let env = RequestEnv {
            config: &self.config,
            catalog: &self.catalog,
            diagnostics: &self.diagnostics,
        };
        // Лента запросов вспышки (14d): какая вспышка и экспозиция ушли в повторяющийся запрос.
        tracing::debug!(target: LOG_TARGET, flash = ?controls.flash, exposure = ?controls.exposure, "повторяющийся запрос");
        apply_repeating_request(&env, device, session, intent, controls)
    }

    fn submit_focus_trigger(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        trigger: AfTrigger,
        request: PointRequestId,
    ) -> Result<(), RequestFailure> {
        self.reap_retired_readers();
        let (Some(device), Some(session)) = (self.device.as_ref(), self.session.as_mut()) else {
            return Err(RequestFailure::NoSession);
        };
        let Some(record) = self.catalog.get(&session.stream().camera) else {
            return Err(RequestFailure::NoSession);
        };
        let planned = plan_request(&RequestInputs {
            intent,
            controls,
            trigger: Some(trigger),
            frame_rate: session.stream().frame_rate,
            mechanism: session.mechanism(),
            camera: &record.descriptor.capabilities,
            mains: self.config.mains_frequency,
        });
        self.diagnostics.record_plan_notes(&planned.notes);
        let one_shot = [OneShotRequest {
            plan: &planned.plan,
            origin: RequestOrigin::FocusTrigger { request },
        }];
        session
            .submit_one_shot(device, &one_shot, &record.vendor)
            .inspect_err(|failure| {
                self.diagnostics
                    .record_failure("submit_focus_trigger", 0, &failure.to_string());
            })
    }

    fn submit_bracket(
        &mut self,
        intent: CaptureIntent,
        base: &RequestControls,
        bracket: &ExposureBracket,
        burst: BurstId,
    ) -> Result<(), RequestFailure> {
        self.reap_retired_readers();
        let (Some(device), Some(session)) = (self.device.as_ref(), self.session.as_mut()) else {
            return Err(RequestFailure::NoSession);
        };
        let Some(record) = self.catalog.get(&session.stream().camera) else {
            return Err(RequestFailure::NoSession);
        };
        let planned: Vec<PlannedRequest> = plan_bracket(
            &RequestInputs {
                intent,
                controls: base,
                trigger: None,
                frame_rate: session.stream().frame_rate,
                mechanism: session.mechanism(),
                camera: &record.descriptor.capabilities,
                mains: self.config.mains_frequency,
            },
            bracket,
        );
        // Кадры серии различаются только экспозицией, поэтому заметки одинаковы — считаем один раз.
        if let Some(first) = planned.first() {
            self.diagnostics.record_plan_notes(&first.notes);
        }
        let one_shots: Vec<OneShotRequest<'_>> = planned
            .iter()
            .zip(0_u32..)
            .map(|(request, index)| OneShotRequest {
                plan: &request.plan,
                origin: RequestOrigin::Burst { burst, index },
            })
            .collect();
        session
            .submit_one_shot(device, &one_shots, &record.vendor)
            .inspect_err(|failure| {
                self.diagnostics
                    .record_failure("submit_bracket", 0, &failure.to_string());
            })
    }

    fn submit_flash_still(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        shot: FlashShotId,
    ) -> Result<(), RequestFailure> {
        self.reap_retired_readers();
        let (Some(device), Some(session)) = (self.device.as_ref(), self.session.as_mut()) else {
            return Err(RequestFailure::NoSession);
        };
        let Some(record) = self.catalog.get(&session.stream().camera) else {
            return Err(RequestFailure::NoSession);
        };
        tracing::debug!(target: LOG_TARGET, ?shot, flash = ?controls.flash, exposure = ?controls.exposure, "разовый запрос со вспышкой");
        // Условия снимка (Single, ручной сенсор, вспышка есть) проверяет чистый планировщик;
        // отказ — честная ошибка запроса, а не молчаливый обычный кадр.
        let planned = plan_flash_still(&RequestInputs {
            intent,
            controls,
            trigger: None,
            frame_rate: session.stream().frame_rate,
            mechanism: session.mechanism(),
            camera: &record.descriptor.capabilities,
            mains: self.config.mains_frequency,
        })
        .map_err(|refusal| {
            let failure = RequestFailure::Backend(BackendFailure {
                operation: "submit_flash_still",
                code: tags::STATUS_INVALID_PARAMETER,
                detail: refusal.to_string(),
            });
            self.diagnostics.record_failure(
                "submit_flash_still",
                tags::STATUS_INVALID_PARAMETER,
                &refusal.to_string(),
            );
            failure
        })?;
        self.diagnostics.record_plan_notes(&planned.notes);
        let one_shot = [OneShotRequest {
            plan: &planned.plan,
            origin: RequestOrigin::FlashStill { shot },
        }];
        session
            .submit_one_shot(device, &one_shot, &record.vendor)
            .inspect_err(|failure| {
                self.diagnostics
                    .record_failure("submit_flash_still", 0, &failure.to_string());
            })
    }

    fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        self.reap_retired_readers();
        let Some(session) = self.session.as_mut() else {
            return Err(RequestFailure::NoSession);
        };
        session.stop_repeating()
    }

    fn close_session(&mut self) {
        self.reap_retired_readers();
        if let Some(session) = self.session.take() {
            tracing::debug!(target: LOG_TARGET, session = session.generation().0, "закрытие сессии");
            self.parked_outputs = Some(session.into_parked_outputs());
        }
    }

    fn standby(&mut self) -> StandbySupport<'_> {
        self.standby_support()
    }

    fn close_device(&mut self) {
        // Порядок: стоп повтора и закрытие сессии → закрытие устройства → отсрочка reader-а (он
        // удалится, когда вернутся все его кадры; поток камеры при этом ничего не ждёт).
        self.close_session();
        self.device = None;
        self.parked_outputs = None;
        self.reap_retired_readers();
    }
}

impl Drop for AndroidCameraBackend {
    fn drop(&mut self) {
        self.close_device();
        // Тёплые закрываются раньше reader-ов: их кадры тоже должны успеть вернуться.
        self.close_all_standby();
        // Единственное место, где поток камеры ждёт возврата кадров от других потоков, и ожидание
        // ограничено: reader нельзя удалять, пока GPU может читать его кадры; не вернулись — утекает.
        let report = self.retired_readers.shutdown(
            self.config.retired_reader_drain_timeout(),
            self.config.retired_reader_poll_interval(),
        );
        if report.leaked > 0 {
            tracing::error!(target: LOG_TARGET, leaked = report.leaked, "при остановке бэкенда reader-ы оставлены: кадры не вернулись");
        }
        if let Some(registration) = self.availability.take() {
            registration.unregister(&self.manager, &self.diagnostics);
        }
        // `onClosed` сессии приходит из её деструктора на потоке последнего владельца и может
        // отработать после этой точки: освободить контексты здесь — use-after-free. Утекают намеренно.
        leak_contexts_for_process_lifetime(std::mem::take(&mut self.retained_state_contexts), &self.diagnostics);
    }
}
