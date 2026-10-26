//! `AndroidCameraBackend` — реализация `CameraBackend` на Camera2 NDK.
//!
//! Тонкий слой: решения принимает нейтральная логика (`frame_rate_plan`, `request_plan`,
//! `characteristics`, `status_mapping`), здесь они только применяются через RAII-владельцы NDK.
//! Все вызовы приходят с потока камеры, колбэки NDK — на чужих потоках и только шлют события.

use std::collections::HashMap;
use std::sync::Arc;

use camera_contract::capabilities::{CameraDescriptor, FocusCapability};
use camera_contract::control::{CaptureIntent, ManualControl};
use camera_contract::ids::CameraId;
use camera_contract::raw_format::{MosaicKind, NativeMosaic};
use camera_contract::stream::{ConfiguredStream, ResolvedStream};
use camera_core::backend::{
    BackendEventSink, BackendFailure, CameraBackend, ConfigureFailure, DeviceGeneration, OpenFailure, RequestFailure,
    SessionGeneration, UnsupportedStream,
};

use crate::characteristics::{CharacteristicsError, parse_characteristics};
use crate::config::AndroidCameraConfig;
use crate::context_leak::leak_contexts_for_process_lifetime;
use crate::diagnostics::BackendDiagnostics;
use crate::entry::{EntryLookup, EntryValue, MetadataSource};
use crate::frame_rate_plan::plan_frame_rate;
use crate::request_plan::{RequestInputs, plan_repeating_request, plan_session_parameters};
use crate::tags::Tag;
use crate::vendor::{VendorCapabilities, VendorKey};

use super::availability::AvailabilityRegistration;
use super::callbacks::SessionStateContext;
use super::device::OpenedDevice;
use super::manager::CameraManager;
use super::raw_sink::TemporaryRawSink;
use super::session::{ActiveSession, SessionSpec};

const LOG_TARGET: &str = "fasticamera::camera::backend";

/// Камера каталога: описание и разрешённые вендорские теги.
struct CameraRecord {
    descriptor: CameraDescriptor,
    vendor: VendorCapabilities,
}

/// Бэкенд камеры на Camera2 NDK.
pub struct AndroidCameraBackend {
    config: AndroidCameraConfig,
    diagnostics: Arc<BackendDiagnostics>,
    events: BackendEventSink,
    catalog: HashMap<CameraId, CameraRecord>,
    // Порядок полей ниже — порядок разрушения как запасной (основной — `Drop`): сессия, приёмник
    // после сессии, затем устройство, затем менеджер.
    session: Option<ActiveSession>,
    /// Приёмник закрытой сессии: живёт, пока устройство не отпустило окно (см. `session`).
    parked_sink: Option<TemporaryRawSink>,
    device: Option<OpenedDevice>,
    /// Контексты состояния сессий: не освобождаются до конца бэкенда, а при его разрушении
    /// намеренно утекают (`Drop`, см. доку `callbacks` и `context_leak`).
    retained_state_contexts: Vec<Arc<SessionStateContext>>,
    availability: Option<AvailabilityRegistration>,
    manager: CameraManager,
}

impl AndroidCameraBackend {
    /// Создаёт бэкенд на потоке камеры: менеджер Camera2 и колбэки доступности.
    pub fn new(
        config: AndroidCameraConfig,
        events: BackendEventSink,
        diagnostics: Arc<BackendDiagnostics>,
    ) -> Result<Self, BackendFailure> {
        config.validate().map_err(|error| BackendFailure {
            operation: "validate_config",
            code: 0,
            detail: error.to_string(),
        })?;
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
            diagnostics,
            events,
            catalog: HashMap::new(),
            session: None,
            parked_sink: None,
            device: None,
            retained_state_contexts: Vec::new(),
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
        let parsed =
            parse_characteristics(&view, native_mosaic).map_err(|error: CharacteristicsError| error.to_string())?;
        self.diagnostics.record_camera(&camera, vendor.report(), parsed.notes);
        Ok(CameraRecord {
            descriptor: CameraDescriptor {
                id: camera,
                capabilities: parsed.capabilities,
            },
            vendor,
        })
    }

    /// Отказ платформы: в диагностику и дальше без изменений.
    fn note_failure(&self, failure: &BackendFailure) {
        self.diagnostics
            .record_failure(failure.operation, failure.code, &failure.detail);
    }

    /// Создание новой сессии (предыдущая уже закрыта вызывающим).
    fn create_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        let Some(device) = self.device.as_mut() else {
            return Err(ConfigureFailure::DeviceDisconnected);
        };
        if device.camera() != &stream.camera {
            return Err(ConfigureFailure::Backend(BackendFailure {
                operation: "configure_session",
                code: 0,
                detail: format!(
                    "устройство открыто для камеры {}, а режим — для {}",
                    device.camera(),
                    stream.camera
                ),
            }));
        }
        let Some(record) = self.catalog.get(&stream.camera) else {
            return Err(ConfigureFailure::Backend(BackendFailure {
                operation: "configure_session",
                code: 0,
                detail: format!("камеры {} нет в прочитанном каталоге", stream.camera),
            }));
        };
        let mechanism = plan_frame_rate(stream, &record.descriptor.capabilities, &record.vendor, &self.config)
            .map_err(|unsupported: UnsupportedStream| {
                tracing::warn!(target: LOG_TARGET, %unsupported, "режим потока недостижим на этой камере");
                ConfigureFailure::Unsupported(unsupported)
            })?;

        let sink = TemporaryRawSink::create(
            stream.buffer_size,
            stream.raw_format,
            self.config.raw_reader_max_images,
            Arc::clone(&self.diagnostics),
        )
        .map_err(ConfigureFailure::Backend)?;
        // Шаблон параметров сессии — PREVIEW: единственный их ключ (диапазон AE) от шаблона не зависит.
        let session_parameters = plan_session_parameters(CaptureIntent::Preview, mechanism);
        let spec = SessionSpec {
            stream,
            mechanism,
            generation,
            session_parameters: session_parameters.as_ref(),
            vendor: &record.vendor,
            frame_number_window: self.config.frame_number_window,
        };
        let (session, state_context) = ActiveSession::create(device, sink, &spec, &self.events, &self.diagnostics)?;
        self.diagnostics.record_retained_session_context();
        self.retained_state_contexts.push(state_context);
        self.session = Some(session);
        tracing::info!(target: LOG_TARGET, camera = %stream.camera, ?mechanism, "сессия сконфигурирована");
        Ok(ConfiguredStream {
            stream: stream.clone(),
            frame_rate_mechanism: mechanism,
        })
    }
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
        // Предыдущую сессию закрываем до создания новой; её приёмник держим, пока устройство не
        // перешло на новую сессию (удалять reader под работающим потоком нельзя).
        let previous_sink = self.session.take().map(ActiveSession::into_parked_sink);
        let outcome = self.create_session(stream, generation);
        if let Some(sink) = previous_sink {
            self.parked_sink = Some(sink);
        }
        if let Err(failure) = &outcome {
            self.diagnostics
                .record_failure("configure_session", 0, &failure.to_string());
        }
        outcome
    }

    fn set_repeating_request(&mut self, intent: CaptureIntent, controls: &ManualControl) -> Result<(), RequestFailure> {
        let (Some(device), Some(session)) = (self.device.as_ref(), self.session.as_mut()) else {
            return Err(RequestFailure::NoSession);
        };
        let Some(record) = self.catalog.get(&session.stream().camera) else {
            return Err(RequestFailure::NoSession);
        };
        let focus: FocusCapability = record.descriptor.capabilities.focus;
        let plan = plan_repeating_request(&RequestInputs {
            intent,
            controls,
            frame_rate: session.stream().frame_rate,
            mechanism: session.mechanism(),
            focus,
        });
        session
            .set_repeating_request(device, &plan, &record.vendor)
            .inspect_err(|failure| {
                self.diagnostics
                    .record_failure("set_repeating_request", 0, &failure.to_string());
            })
    }

    fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        let Some(session) = self.session.as_mut() else {
            return Err(RequestFailure::NoSession);
        };
        session.stop_repeating()
    }

    fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            tracing::debug!(target: LOG_TARGET, session = session.generation().0, "закрытие сессии");
            self.parked_sink = Some(session.into_parked_sink());
        }
    }

    fn close_device(&mut self) {
        // Порядок: стоп повтора и закрытие сессии → закрытие устройства → удаление reader-а.
        self.close_session();
        self.device = None;
        self.parked_sink = None;
    }
}

impl Drop for AndroidCameraBackend {
    fn drop(&mut self) {
        self.close_device();
        if let Some(registration) = self.availability.take() {
            registration.unregister(&self.manager, &self.diagnostics);
        }
        // `onClosed` сессии приходит из её деструктора на потоке последнего владельца и может
        // отработать после этой точки: освободить контексты здесь — use-after-free. Утекают намеренно.
        leak_contexts_for_process_lifetime(std::mem::take(&mut self.retained_state_contexts), &self.diagnostics);
    }
}
