//! Состояние оболочки на процесс и её установка.
//!
//! JNI-колбэки — статические точки входа, поэтому состояние оболочки живёт в одном
//! процессном [`ShellRuntime`] (`OnceLock`). Его ставит composition root (`install`) при
//! инициализации нативной части; до этого колбэки только считают «оболочка не установлена».
//!
//! Владение:
//! * очередь событий — отправитель здесь, получатель отдаётся маршрутизатору в [`ShellHandles`];
//! * своя ссылка на текущее окно — в [`SurfaceRegistry`] (для частоты кадров);
//! * Kotlin-объект платформы — глобальная ссылка JNI на время жизни Activity (подсчитывается).

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use jni::JavaVM;
use jni::objects::{Global, JObject};
use shell_contract::commands::PlatformCommandFailure;
use shell_contract::diagnostics::ShellCounters;
use shell_contract::event::ShellEvent;
use shell_contract::event_queue::{
    SendOutcome, ShellEventReceiver, ShellEventSender, ShellQueueConfigError, shell_event_channel,
};

use super::native_window::NativeWindowRef;
use super::platform_calls::{PlatformCallSender, platform_call_channel, start_platform_call_thread};
use super::services::AndroidShellServices;
use super::thermal_monitor::{ThermalMonitor, start_thermal_monitor};
use crate::accounting::ShellAccounting;
use crate::config::{ShellConfig, ShellConfigError, ValidatedShellConfig};
use crate::haptic_plan::{HapticCapabilities, HapticTuning};
use crate::surface_registry::SurfaceRegistry;

static SHELL_RUNTIME: OnceLock<Arc<ShellRuntime>> = OnceLock::new();

/// Оболочка процесса, если она установлена.
pub fn installed_runtime() -> Option<&'static Arc<ShellRuntime>> {
    SHELL_RUNTIME.get()
}

/// Что получает composition root после установки.
pub struct ShellHandles {
    /// Единственный получатель событий оболочки (маршрутизатор).
    pub events: ShellEventReceiver,
    /// Реализации трейтов `shell-contract` для потребителей.
    pub services: Arc<AndroidShellServices>,
}

/// Почему оболочка не установлена.
#[derive(Debug, thiserror::Error)]
pub enum ShellInstallError {
    #[error("оболочка уже установлена в этом процессе")]
    AlreadyInstalled,
    #[error("неверные настройки оболочки: {0}")]
    Config(#[from] ShellConfigError),
    #[error("неверные ёмкости очереди событий: {0}")]
    Queue(#[from] ShellQueueConfigError),
    #[error("JavaVM не известна: JNI_OnLoad не вызывал JavaVM::from_raw ({0})")]
    NoJavaVm(jni::errors::Error),
    #[error("поток команд платформы не создан: {0}")]
    CommandThread(std::io::Error),
}

/// Устанавливает оболочку процесса: очередь событий, поток команд, тепловой монитор.
pub fn install(config: &ShellConfig) -> Result<ShellHandles, ShellInstallError> {
    if SHELL_RUNTIME.get().is_some() {
        return Err(ShellInstallError::AlreadyInstalled);
    }
    let validated = config.validate()?;
    let vm = JavaVM::singleton().map_err(ShellInstallError::NoJavaVm)?;
    let (events, receiver) = shell_event_channel(validated.queue)?;
    let (calls, call_receiver) = platform_call_channel(validated.command_queue_capacity);
    let thermal = start_thermal_monitor(validated.thermal_poll_interval, events.clone());
    let runtime = Arc::new(ShellRuntime {
        config: validated,
        events,
        accounting: Arc::new(ShellAccounting::new()),
        surfaces: Mutex::new(SurfaceRegistry::default()),
        platform: Mutex::new(None),
        calls,
        haptic_tuning: HapticTuning::default(),
        thermal,
        gyro_running: AtomicBool::new(false),
        orientation_running: AtomicBool::new(false),
        power_sampling_running: AtomicBool::new(false),
    });
    // Сначала все потоки, потом публикация: провал на полпути не оставляет «установленную»
    // оболочку без потока команд, и initialize можно повторить. Уже запущенный тепловой
    // монитор при этом сам остановится: его получатель событий уничтожится вместе с `receiver`.
    start_platform_call_thread(vm, Arc::clone(&runtime), call_receiver).map_err(ShellInstallError::CommandThread)?;
    // Гонки нет: install вызывается из initialize на главном потоке Android.
    SHELL_RUNTIME
        .set(Arc::clone(&runtime))
        .map_err(|_| ShellInstallError::AlreadyInstalled)?;
    tracing::info!(target: "fasticamera::shell", "оболочка установлена");
    Ok(ShellHandles {
        events: receiver,
        services: Arc::new(AndroidShellServices::new(runtime)),
    })
}

/// Глобальная ссылка JNI, которая учитывается в счётчике живых ссылок.
pub struct CountedGlobal {
    global: Global<JObject<'static>>,
    accounting: Arc<ShellAccounting>,
}

impl CountedGlobal {
    pub fn new(global: Global<JObject<'static>>, accounting: Arc<ShellAccounting>) -> Self {
        accounting.record_global_ref_created();
        Self { global, accounting }
    }

    pub fn object(&self) -> &JObject<'static> {
        &self.global
    }
}

impl Drop for CountedGlobal {
    fn drop(&mut self) {
        // Сама ссылка удаляется сразу после этого (Drop поля Global → DeleteGlobalRef).
        self.accounting.record_global_ref_deleted();
    }
}

/// Подключённая Activity: Kotlin-объект платформы и то, что о ней узнали при подключении.
pub struct AttachedPlatform {
    object: CountedGlobal,
    haptic_capabilities: HapticCapabilities,
    package_name: CString,
}

impl AttachedPlatform {
    pub fn new(object: CountedGlobal, haptic_capabilities: HapticCapabilities, package_name: CString) -> Self {
        Self {
            object,
            haptic_capabilities,
            package_name,
        }
    }

    pub fn object(&self) -> &JObject<'static> {
        self.object.object()
    }

    pub fn haptic_capabilities(&self) -> &HapticCapabilities {
        &self.haptic_capabilities
    }

    pub fn package_name(&self) -> &CString {
        &self.package_name
    }
}

/// Оболочка процесса.
pub struct ShellRuntime {
    config: ValidatedShellConfig,
    events: ShellEventSender,
    accounting: Arc<ShellAccounting>,
    surfaces: Mutex<SurfaceRegistry<NativeWindowRef>>,
    platform: Mutex<Option<Arc<AttachedPlatform>>>,
    calls: PlatformCallSender,
    haptic_tuning: HapticTuning,
    thermal: ThermalMonitor,
    gyro_running: AtomicBool,
    orientation_running: AtomicBool,
    power_sampling_running: AtomicBool,
}

impl ShellRuntime {
    pub fn config(&self) -> &ValidatedShellConfig {
        &self.config
    }

    pub fn accounting(&self) -> &Arc<ShellAccounting> {
        &self.accounting
    }

    pub fn calls(&self) -> &PlatformCallSender {
        &self.calls
    }

    pub fn haptic_tuning(&self) -> &HapticTuning {
        &self.haptic_tuning
    }

    pub fn thermal(&self) -> &ThermalMonitor {
        &self.thermal
    }

    pub fn events(&self) -> &ShellEventSender {
        &self.events
    }

    /// Отправляет событие; потеря критичного события — ошибка в логе (счётчик ведёт очередь).
    pub fn send_event(&self, event: ShellEvent) -> SendOutcome {
        let kind = event.kind();
        let outcome = self.events.send(event);
        match outcome {
            SendOutcome::Delivered | SendOutcome::TouchDropped => {}
            SendOutcome::CriticalLost => {
                tracing::error!(target: "fasticamera::shell", event = kind.name(), "очередь событий полна: событие потеряно");
            }
            SendOutcome::ReceiverGone => {
                tracing::warn!(target: "fasticamera::shell", event = kind.name(), "маршрутизатор событий не работает");
            }
        }
        outcome
    }

    pub fn surfaces(&self) -> MutexGuard<'_, SurfaceRegistry<NativeWindowRef>> {
        // Под замком только операции реестра без паник; после чужой паники состояние согласовано.
        self.surfaces.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Текущая подключённая Activity (клон `Arc`: замок не держится во время вызова в Java).
    pub fn attached_platform(&self) -> Option<Arc<AttachedPlatform>> {
        self.lock_platform().clone()
    }

    /// Подключает Activity; прежняя (если была) отключается.
    pub fn attach_platform(&self, platform: AttachedPlatform) {
        self.accounting.record_activity_attached();
        let previous = self.lock_platform().replace(Arc::new(platform));
        if previous.is_some() {
            tracing::warn!(target: "fasticamera::shell", "новая Activity подключилась раньше, чем отключилась прежняя");
        }
    }

    /// Отключает Activity, если подключена именно она (`is_current` сравнивает объекты в JNI).
    /// Возвращает, была ли она отключена.
    pub fn detach_platform_if(&self, is_current: impl FnOnce(&AttachedPlatform) -> bool) -> bool {
        let mut slot = self.lock_platform();
        let matches = slot.as_deref().is_some_and(is_current);
        if matches {
            // Ссылку отпускаем вне замка: Drop глобальной ссылки — вызов в JVM.
            let detached = slot.take();
            drop(slot);
            drop(detached);
        }
        matches
    }

    /// Отказ платформы выполнить команду: счётчик, лог и событие для потребителей.
    pub fn report_platform_failure(&self, failure: PlatformCommandFailure) {
        self.accounting.record_platform_command_failure();
        tracing::warn!(
            target: "fasticamera::shell",
            command = ?failure.command,
            reason = %failure.reason,
            "команда платформы не выполнена"
        );
        self.send_event(ShellEvent::PlatformCommandFailed(failure));
    }

    pub fn counters(&self) -> ShellCounters {
        self.accounting.snapshot(self.events.stats())
    }

    /// Флаг «датчик занят» (гироскоп, ориентация, опрос батареи): `true`, если удалось занять.
    pub fn try_claim_sensor(&self, sensor: ClaimedSensor) -> bool {
        self.sensor_flag(sensor)
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn release_sensor(&self, sensor: ClaimedSensor) {
        self.sensor_flag(sensor).store(false, Ordering::Release);
    }

    fn sensor_flag(&self, sensor: ClaimedSensor) -> &AtomicBool {
        match sensor {
            ClaimedSensor::Gyro => &self.gyro_running,
            ClaimedSensor::Orientation => &self.orientation_running,
            ClaimedSensor::BatteryPower => &self.power_sampling_running,
        }
    }

    fn lock_platform(&self) -> MutexGuard<'_, Option<Arc<AttachedPlatform>>> {
        // Под замком только замена Option; паника внутри невозможна.
        self.platform.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Датчики, у которых может быть только одна подписка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimedSensor {
    Gyro,
    Orientation,
    /// Опрос батареи (`BatteryManager`) — «датчик тока»: второй опрос удвоил бы binder-вызовы.
    BatteryPower,
}
