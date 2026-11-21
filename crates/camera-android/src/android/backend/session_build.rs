//! Общая сборка сессии и повторяющегося запроса для основной и тёплых камер (13b).
//!
//! Тёплая камера планируется **ровно так же**, как основная: тот же `plan_frame_rate`
//! (вендорские ключи CamX только на перечисленных камерах, AE `[fps, fps]` там, где режимы
//! сенсора позволяют), тот же `plan_request`. Различаются только роль сессии (куда идут её
//! результаты) и вид эпохи потока кадров (показываемая/тёплая) — оба приходят параметром.

use std::collections::HashMap;
use std::sync::Arc;

use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, ResolvedStream};
use camera_core::backend::{
    BackendEventSink, BackendFailure, ConfigureFailure, RequestFailure, SessionGeneration, UnsupportedStream,
};
use camera_core::request_controls::RequestControls;
use raw_frame_stream::RawFrameProducer;

use crate::config::AndroidCameraConfig;
use crate::cpu_access_probe::CpuAccessProbe;
use crate::diagnostics::BackendDiagnostics;
use crate::frame_rate_plan::plan_frame_rate;
use crate::request_plan::{RequestInputs, plan_request, plan_session_parameters};
use crate::result_routing::CaptureRole;
use crate::stream_passport::build_stream_passport;

use super::{CameraRecord, LOG_TARGET};
use crate::android::callbacks::SessionStateContext;
use crate::android::device::OpenedDevice;
use crate::android::raw_reader::{RawReader, ReaderEpochKind, ReaderSpec, RetiredNdkReaders};
use crate::android::session::{ActiveSession, SessionSpec};

/// Всё, что сборке сессии нужно от бэкенда, кроме устройства (у каждой линии оно своё).
pub(super) struct SessionEnv<'a> {
    pub config: &'a AndroidCameraConfig,
    pub catalog: &'a HashMap<CameraId, CameraRecord>,
    pub events: &'a BackendEventSink,
    pub diagnostics: &'a Arc<BackendDiagnostics>,
    pub producer: &'a RawFrameProducer,
    pub cpu_probe: &'a CpuAccessProbe,
    pub retired_readers: &'a Arc<RetiredNdkReaders>,
}

/// Собранная сессия: сама сессия, контекст её состояния (бэкенд хранит его до конца жизни) и итог для движка.
pub(super) struct BuiltSession {
    pub session: ActiveSession,
    pub state_context: Arc<SessionStateContext>,
    pub configured: ConfiguredStream,
}

/// Создаёт сессию на открытом устройстве `device` для режима `stream`.
///
/// `role` определяет и вид эпохи потока кадров: основная — показываемая (`begin_stream`),
/// тёплая — тёплая эпоха с **полным** пулом (после обмена ролей reader станет основным).
pub(super) fn build_session(
    env: &SessionEnv<'_>,
    device: &mut OpenedDevice,
    stream: &ResolvedStream,
    generation: SessionGeneration,
    role: CaptureRole,
) -> Result<BuiltSession, ConfigureFailure> {
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
    let Some(record) = env.catalog.get(&stream.camera) else {
        return Err(ConfigureFailure::Backend(BackendFailure {
            operation: "configure_session",
            code: 0,
            detail: format!("камеры {} нет в прочитанном каталоге", stream.camera),
        }));
    };
    let mechanism = plan_frame_rate(
        stream,
        &record.descriptor.capabilities,
        &record.vendor,
        &record.sensor_modes,
        env.config,
    )
    .map_err(|unsupported: UnsupportedStream| {
        tracing::warn!(target: LOG_TARGET, %unsupported, "режим потока недостижим на этой камере");
        ConfigureFailure::Unsupported(unsupported)
    })?;

    // Паспорт постоянной части: без него поток кадров не стартует (RAW не поддержан / полоса 16:9
    // не помещается — это отказ режима, а не сбой платформы).
    let passport = build_stream_passport(stream, &record.descriptor.capabilities, mechanism).map_err(|error| {
        ConfigureFailure::Backend(BackendFailure {
            operation: "build_stream_passport",
            code: 0,
            detail: error.to_string(),
        })
    })?;
    let reader = RawReader::create(ReaderSpec {
        epoch_kind: match role {
            CaptureRole::Primary => ReaderEpochKind::Shown,
            CaptureRole::Standby(_) => ReaderEpochKind::Standby,
        },
        size: stream.buffer_size,
        format: stream.raw_format,
        passport: Arc::new(passport),
        producer: env.producer.clone(),
        probe: env.cpu_probe.clone(),
        diagnostics: Arc::clone(env.diagnostics),
        retirement: Arc::clone(env.retired_readers),
    })
    .map_err(ConfigureFailure::Backend)?;
    // Шаблон параметров сессии — PREVIEW: единственный их ключ (диапазон AE) от шаблона не зависит.
    let session_parameters = plan_session_parameters(CaptureIntent::Preview, mechanism);
    let spec = SessionSpec {
        stream,
        mechanism,
        generation,
        session_parameters: session_parameters.as_ref(),
        vendor: &record.vendor,
        frame_number_window: env.config.frame_number_window,
        producer: env.producer,
        companion: env.config.three_a_companion_stream.plan(),
        origin_capacity: env.config.request_origin_capacity,
        role,
    };
    let (session, state_context) = ActiveSession::create(device, reader, &spec, env.events, env.diagnostics)?;
    tracing::info!(target: LOG_TARGET, camera = %stream.camera, ?mechanism, ?role, "сессия сконфигурирована");
    Ok(BuiltSession {
        session,
        state_context,
        configured: ConfiguredStream {
            stream: stream.clone(),
            frame_rate_mechanism: mechanism,
        },
    })
}

/// Что нужно, чтобы спланировать запрос камеры.
pub(super) struct RequestEnv<'a> {
    pub config: &'a AndroidCameraConfig,
    pub catalog: &'a HashMap<CameraId, CameraRecord>,
    pub diagnostics: &'a Arc<BackendDiagnostics>,
}

/// Планирует и ставит повторяющийся запрос сессии: один путь для основной и тёплой камер.
pub(super) fn apply_repeating_request(
    env: &RequestEnv<'_>,
    device: &OpenedDevice,
    session: &mut ActiveSession,
    intent: CaptureIntent,
    controls: &RequestControls,
) -> Result<(), RequestFailure> {
    let Some(record) = env.catalog.get(&session.stream().camera) else {
        return Err(RequestFailure::NoSession);
    };
    let planned = plan_request(&RequestInputs {
        intent,
        controls,
        trigger: None,
        frame_rate: session.stream().frame_rate,
        mechanism: session.mechanism(),
        camera: &record.descriptor.capabilities,
        mains: env.config.mains_frequency,
    });
    env.diagnostics.record_plan_notes(&planned.notes);
    session
        .set_repeating_request(device, &planned.plan, &record.vendor)
        .inspect_err(|failure| {
            env.diagnostics
                .record_failure("set_repeating_request", 0, &failure.to_string());
        })
}
