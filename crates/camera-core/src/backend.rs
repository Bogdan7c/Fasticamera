//! Граница движка с платформой: трейт [`CameraBackend`] и события бэкенда.
//!
//! Правило потоков: бэкенд создаётся фабрикой **на потоке камеры** и живёт только там, поэтому
//! трейт не требует `Send` (объекты NDK не обязаны быть переносимыми). Методы вызывает только
//! движок и только с этого потока; они могут блокировать (NDK `openCamera` и
//! `createCaptureSession` синхронны и занимают сотни миллисекунд) — это допустимо, потому что
//! поток камеры никто не ждёт (инвариант 4 README).
//!
//! Асинхронное (отключение устройства, ошибки HAL, результаты запросов, свобода камеры)
//! бэкенд отдаёт через [`BackendEventSink`]. Колбэки NDK приходят на чужих потоках binder,
//! поэтому `deliver` никогда не блокирует и ничего не ждёт.
//!
//! Поколения ([`DeviceGeneration`], [`SessionGeneration`]) выдаёт движок при каждом открытии
//! и конфигурации. Бэкенд ставит их на события, а движок отбрасывает события прошлых
//! устройств и сессий (и считает их): запоздалое «отключено» от закрытого устройства
//! не должно убить новое.

use std::sync::Arc;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capabilities::CameraDescriptor;
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber};
use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ConfiguredStream, ReadoutAspect, ResolvedStream, TargetFrameRate};
use camera_contract::three_a::{BurstId, ExposureBracket, PointRequestId, RequestOrigin};
use serde::Serialize;

use crate::request_controls::{AfTrigger, RequestControls};

/// Поколение открытого устройства. Растёт на каждое `open_device`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct DeviceGeneration(pub u64);

/// Поколение сессии. Растёт на каждое `configure_session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SessionGeneration(pub u64);

/// Сбой вызова платформы, который не укладывается в смысловые варианты: код и контекст
/// сохраняются для отчёта, а не теряются.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{operation}: код {code}: {detail}")]
pub struct BackendFailure {
    /// Что вызывали (например, `ACameraManager_getCameraIdList`).
    pub operation: &'static str,
    /// Код платформы (`camera_status_t`, `media_status_t`); 0, если кода нет.
    pub code: i32,
    pub detail: String,
}

/// Почему не открылось устройство.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OpenFailure {
    /// Камера недоступна по понятной причине (занята, нет разрешения, запрещена политикой…).
    #[error("камера недоступна: {}", reason.name())]
    Unavailable { reason: CameraUnavailableReason },
    #[error(transparent)]
    Backend(BackendFailure),
}

/// Что из режима потока бэкенд не может дать на этой камере.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UnsupportedStream {
    #[error("формат {} не предлагается камерой", format.name())]
    FormatNotOffered { format: RawFormat },
    #[error("вырезка {readout:?} недоступна на этой камере")]
    ReadoutNotAvailable { readout: ReadoutAspect },
    #[error("частота {} к/с недостижима известными рычагами", requested.frames_per_second())]
    FrameRateNotAchievable { requested: TargetFrameRate },
}

/// Почему не сконфигурировалась сессия.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ConfigureFailure {
    /// Устройство отключилось во время конфигурации (вытеснение на `Configuring`).
    #[error("устройство отключилось во время конфигурации")]
    DeviceDisconnected,
    /// Бэкенд заранее знает, что режим невозможен.
    #[error(transparent)]
    Unsupported(UnsupportedStream),
    /// HAL отверг комбинацию потоков (`ACAMERA_ERROR_STREAM_CONFIGURE_FAIL`).
    #[error("HAL отверг конфигурацию потоков: {detail}")]
    RejectedByHal { detail: String },
    #[error(transparent)]
    Backend(BackendFailure),
}

/// Почему не принят повторяющийся запрос.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RequestFailure {
    #[error("устройство отключилось")]
    DeviceDisconnected,
    /// Сессии нет или она уже закрыта (вызов не в том состоянии — ошибка движка).
    #[error("нет активной сессии")]
    NoSession,
    #[error(transparent)]
    Backend(BackendFailure),
}

/// Код ошибки устройства из `ACameraDevice_ErrorStateCallback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "code")]
pub enum DeviceErrorKind {
    CameraInUse,
    MaxCamerasInUse,
    CameraDisabled,
    CameraDevice,
    CameraService,
    Unknown(i32),
}

/// Свободна ли камера для открытия (`ACameraManager_AvailabilityCallbacks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraAvailability {
    Available,
    Unavailable,
}

/// Почему запрос кадра провалился (`ACameraCaptureFailure::reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureFailureReason {
    /// Ошибка HAL на этом кадре.
    Error,
    /// Запрос сброшен (`abortCaptures`, остановка сессии).
    Flushed,
}

/// Асинхронное событие бэкенда.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BackendEvent {
    /// `onDisconnected`: камеру отняли (вытеснение), отозвали разрешение или её отключили.
    DeviceDisconnected { device: DeviceGeneration },
    /// `onError`: серьёзная ошибка устройства или службы.
    DeviceError {
        device: DeviceGeneration,
        error: DeviceErrorKind,
    },
    /// Сессия закрыта (своим закрытием, новой сессией или отключением устройства).
    SessionClosed { session: SessionGeneration },
    /// Результат запроса пришёл целиком (`onCaptureCompleted`).
    CaptureCompleted {
        session: SessionGeneration,
        result: CaptureResultMeta,
    },
    /// Запрос кадра провалился (`onCaptureFailed`).
    CaptureFailed {
        session: SessionGeneration,
        frame_number: FrameNumber,
        reason: CaptureFailureReason,
    },
    /// Камера стала свободной или занятой для приложения.
    CameraAvailability {
        camera: CameraId,
        availability: CameraAvailability,
    },
}

impl BackendEvent {
    /// Частое событие потока (результаты **повторяющегося** запроса), а не редкое событие.
    ///
    /// Получатель вправе вытеснять частые события при переполнении (со счётчиком), а редкие —
    /// никогда. Результаты разовых запросов (триггер фокуса, кадры серии) —
    /// **редкие**: такой результат единственный на свой запрос, его потеря ломает тап или
    /// серию без возможности понять почему (сессия 08). Их мало: десятки, а не 60 в секунду.
    pub fn is_capture_stream_event(&self) -> bool {
        match self {
            BackendEvent::CaptureCompleted { result, .. } => matches!(
                result.three_a.origin,
                // `Unknown` — тоже частое: если бэкенд не опознаёт запросы вообще, поток таких
                // результатов шёл бы 60 раз в секунду в ящик без лимита.
                RequestOrigin::Repeating | RequestOrigin::Unknown
            ),
            BackendEvent::CaptureFailed { .. } => true,
            _ => false,
        }
    }
}

/// Получатель событий бэкенда. Реализует движок (почтовый ящик потока камеры)
/// или тест. `deliver` обязан вернуться сразу: его зовут колбэки NDK на потоках binder.
pub trait BackendEventListener: Send + Sync {
    fn deliver(&self, event: BackendEvent);
}

/// Ручка, через которую бэкенд отдаёт события. Дёшево клонируется.
#[derive(Clone)]
pub struct BackendEventSink {
    listener: Arc<dyn BackendEventListener>,
}

impl BackendEventSink {
    pub fn new(listener: Arc<dyn BackendEventListener>) -> Self {
        Self { listener }
    }

    pub fn deliver(&self, event: BackendEvent) {
        self.listener.deliver(event);
    }
}

impl std::fmt::Debug for BackendEventSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BackendEventSink")
    }
}

/// Фабрика бэкенда: вызывается один раз **на потоке камеры** с ручкой событий.
pub type BackendFactory =
    Box<dyn FnOnce(BackendEventSink) -> Result<Box<dyn CameraBackend>, BackendFailure> + Send + 'static>;

/// Платформенная реализация камеры: Camera2 NDK (`camera-android`) или fake для тестов.
///
/// Контракт для реализаций:
/// * вызовы приходят только с потока камеры, по одному;
/// * `open_device` при уже открытом устройстве сначала закрывает старое (движок так не делает,
///   но реализация не должна течь);
/// * `close_session` и `close_device` идемпотентны: без сессии/устройства — no-op;
///   `close_device` закрывает и сессию;
/// * после `close_device` события этого поколения больше не приходят или приходят
///   с прежним поколением (движок их отбросит);
/// * ни один метод не паникует на отказе платформы — отказ возвращается вариантом.
pub trait CameraBackend {
    /// Каталог камер, видимых приложению, с возможностями. Роли назначает движок.
    fn load_catalog(&mut self) -> Result<Vec<CameraDescriptor>, BackendFailure>;

    /// Открыть устройство; события устройства помечать `generation`.
    fn open_device(&mut self, camera: &CameraId, generation: DeviceGeneration) -> Result<(), OpenFailure>;

    /// Создать сессию под поток на открытом устройстве; события помечать `generation`.
    /// Возвращает, каким рычагом бэкенд просил частоту.
    fn configure_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure>;

    /// Поставить (или заменить) повторяющийся запрос с этими намерением и планом управления.
    /// Результаты его кадров помечаются `RequestOrigin::Repeating`.
    fn set_repeating_request(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure>;

    /// Отправить **один** запрос с триггером автофокуса поверх идущего повторяющегося
    /// (`ACameraCaptureSession_capture`). Тот же рычаг частоты и вендорские ключи, что у потока.
    /// Результат этого кадра помечается `RequestOrigin::FocusTrigger { request }`.
    fn submit_focus_trigger(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        trigger: AfTrigger,
        request: PointRequestId,
    ) -> Result<(), RequestFailure>;

    /// Отправить серию запросов (`ACameraCaptureSession_capture` с N запросами) поверх
    /// повторяющегося: кадр `i` — с экспозицией `bracket.frames()[i]` (`ExposurePlan::Sensor`),
    /// фокус — из `base`. Результаты помечаются `RequestOrigin::Burst { burst, index }`.
    fn submit_bracket(
        &mut self,
        intent: CaptureIntent,
        base: &RequestControls,
        bracket: &ExposureBracket,
        burst: BurstId,
    ) -> Result<(), RequestFailure>;

    /// Остановить повторяющийся запрос; сессия остаётся.
    fn stop_repeating(&mut self) -> Result<(), RequestFailure>;

    /// Закрыть сессию, если есть.
    fn close_session(&mut self);

    /// Закрыть устройство (и сессию), если открыто.
    fn close_device(&mut self);
}
