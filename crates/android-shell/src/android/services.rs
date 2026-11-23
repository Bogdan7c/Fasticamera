//! Реализации трейтов `shell-contract` поверх оболочки процесса.
//!
//! Потребители (экран камеры, маршрутизатор, сценарии) видят только трейты; этот тип —
//! единственная реализация для телефона. Все методы неблокирующие: команды уходят в очередь
//! потока платформы, датчики и опрос батареи работают в своих потоках.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use shell_contract::clock::BoottimeNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::diagnostics::{ShellCounters, ShellDiagnostics};
use shell_contract::event::ShellEvent;
use shell_contract::gravity::{GravitySlot, GravitySource, GravitySubscription};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticsService};
use shell_contract::motion::{
    AngularVelocity, GyroRequest, GyroSample, GyroSubscription, MotionRing, MotionSource, OrientationSubscription,
    SensorSourceError,
};
use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptService};
use shell_contract::power::{PowerSampleRing, PowerSampling, PowerSamplingRequest, PowerSource, PowerSourceError};
use shell_contract::still_media::ShutterSoundService;
use shell_contract::text_input::{TextInputService, TextRequest};
use shell_contract::thermal::{HeadroomForecast, ThermalHeadroom, ThermalSource, ThermalSourceError, ThermalStatus};
use shell_contract::vsync::{VsyncSlot, VsyncSource, VsyncSourceError, VsyncSubscription};

use super::choreographer_thread::start_choreographer_thread;
use super::platform_calls::PlatformCall;
use super::power_sampler::{PowerSamplerThread, start_power_sampler};
use super::runtime::{ClaimedSensor, ShellRuntime};
use super::sensor_thread::{SensorThread, SensorThreadSpec, event_vector, start_sensor_thread};
use super::thermal_monitor::ThermalListenerState;
use crate::detached_worker::ClaimRelease;
use crate::gravity_source::GravitySettings;
use crate::gravity_source::{GravityIntake, GravityIntakeOutcome, start_with_fallback};
use crate::haptic_plan::{HapticCapabilities, plan_haptic};
use crate::orientation_classifier::{AccelerationSample, OrientationClassifier};
use crate::surface_registry::frame_rate_outcome;

const GYRO_SENSOR: &str = "gyroscope";
const ACCELEROMETER_SENSOR: &str = "accelerometer";
const GRAVITY_SENSOR: &str = "gravity";

/// Сервисы оболочки для потребителей.
pub struct AndroidShellServices {
    runtime: Arc<ShellRuntime>,
}

impl AndroidShellServices {
    pub(crate) fn new(runtime: Arc<ShellRuntime>) -> Self {
        Self { runtime }
    }

    /// Оболочка процесса — для сервисов снимка этого crate (`still_*`), которым нужен JNI-доступ.
    pub(crate) fn runtime(&self) -> &Arc<ShellRuntime> {
        &self.runtime
    }

    /// Запрос гироскопа по настройкам (частота и окно истории из конфига).
    pub fn default_gyro_request(&self) -> GyroRequest {
        self.runtime.config().default_gyro_request
    }

    /// Возможности вибромотора подключённой Activity (`None` — Activity нет).
    pub fn haptic_capabilities(&self) -> Option<HapticCapabilities> {
        self.runtime
            .attached_platform()
            .map(|platform| platform.haptic_capabilities().clone())
    }

    /// Состояние подписки теплового монитора.
    pub fn thermal_listener_state(&self) -> ThermalListenerState {
        self.runtime.thermal().listener_state().clone()
    }

    fn enqueue_for_attached(&self, call: PlatformCall) -> CommandOutcome {
        if self.runtime.attached_platform().is_none() {
            return CommandOutcome::NotAttached;
        }
        self.runtime.calls().enqueue(call)
    }

    /// Имя пакета процесса для `ASensorManager_getInstanceForPackage`. Известно с установки оболочки, поэтому
    /// датчики не ждут подключения Activity (решение 106: раньше экран камеры получал здесь отказ на старте).
    fn package_name(&self) -> std::ffi::CString {
        self.runtime.package_name().as_c_str().to_owned()
    }
}

impl ShellCommands for AndroidShellServices {
    fn request_camera_permission(&self) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::RequestCameraPermission)
    }

    fn set_preferred_frame_rate(&self, rate: FrameRate) -> FrameRateOutcome {
        // Замок реестра снимается в конце этой строки: binder-вызов ниже идёт уже без него.
        let current_window = self.runtime.surfaces().set_preferred_frame_rate(rate);
        match current_window {
            Some(window) => frame_rate_outcome(window.set_frame_rate(rate)),
            None => FrameRateOutcome::DeferredUntilSurface,
        }
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::OpenSystemGallery)
    }

    fn keep_screen_on(&self, wake: ScreenWake) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::SetKeepScreenOn(wake))
    }
}

impl HapticsService for AndroidShellServices {
    fn play(&self, pattern: HapticPattern) -> HapticOutcome {
        let Some(platform) = self.runtime.attached_platform() else {
            return HapticOutcome::NotAttached;
        };
        let Some(realization) = plan_haptic(pattern, platform.haptic_capabilities(), self.runtime.haptic_tuning())
        else {
            return HapticOutcome::Unsupported;
        };
        let kind = realization.kind();
        match self.runtime.calls().enqueue(PlatformCall::PlayHaptic(realization)) {
            CommandOutcome::Queued => HapticOutcome::Queued(kind),
            CommandOutcome::Backpressure => HapticOutcome::Backpressure,
            CommandOutcome::NotAttached => HapticOutcome::NotAttached,
        }
    }
}

impl OperatorPromptService for AndroidShellServices {
    fn show(&self, prompt: &OperatorPrompt) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::ShowOperatorPrompt(prompt.clone()))
    }

    fn clear(&self) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::ClearOperatorPrompt)
    }
}

impl ShutterSoundService for AndroidShellServices {
    /// Звук затвора: команда в очередь потока платформы (как хаптика), главный поток и рендер не ждут.
    /// Отказ самой платформы приходит позже событием `PlatformCommandFailed(PlayShutterSound)`.
    fn play_shutter_sound(&self) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::PlayShutterSound)
    }
}

impl TextInputService for AndroidShellServices {
    /// Диалог ввода текста: команда в очередь потока платформы, ответ придёт событием `ShellEvent::TextInput`
    /// (сбой показа внутри Kotlin — тем же событием с итогом `Unavailable`).
    fn request_text(&self, request: &TextRequest) -> CommandOutcome {
        self.enqueue_for_attached(PlatformCall::RequestText(request.clone()))
    }
}

/// Держит поток датчика и снимает флаг занятости при уничтожении подписки.
struct SensorLease {
    // Порядок полей важен: сначала останавливаем поток, потом освобождаем флаг.
    thread: Option<SensorThread>,
    runtime: Arc<ShellRuntime>,
    sensor: ClaimedSensor,
}

impl Drop for SensorLease {
    fn drop(&mut self) {
        drop(self.thread.take());
        self.runtime.release_sensor(self.sensor);
    }
}

impl AndroidShellServices {
    /// Занимает датчик и запускает поток; при неудаче флаг освобождается.
    fn start_claimed_sensor(
        &self,
        sensor: ClaimedSensor,
        sensor_name: &'static str,
        start: impl FnOnce() -> Result<SensorThread, SensorSourceError>,
    ) -> Result<SensorLease, SensorSourceError> {
        if !self.runtime.try_claim_sensor(sensor) {
            return Err(SensorSourceError::AlreadyRunning { sensor: sensor_name });
        }
        // Флаг уже занят: аренда освободит его и при ошибке запуска (thread = None).
        let mut lease = SensorLease {
            thread: None,
            runtime: Arc::clone(&self.runtime),
            sensor,
        };
        lease.thread = Some(start()?);
        Ok(lease)
    }
}

impl MotionSource for AndroidShellServices {
    fn start_gyro(&self, request: GyroRequest) -> Result<GyroSubscription, SensorSourceError> {
        let package_name = self.package_name();
        let ring = Arc::new(MotionRing::new(request.ring_capacity()));
        let writer = Arc::clone(&ring);
        let lease = self.start_claimed_sensor(ClaimedSensor::Gyro, GYRO_SENSOR, || {
            let spec = SensorThreadSpec {
                sensor_name: GYRO_SENSOR,
                // Калиброванный гироскоп (как в сессии 00, 402 Гц). Метки — CLOCK_BOOTTIME.
                sensor_type: ndk_sys::ASENSOR_TYPE_GYROSCOPE,
                sampling_period: request.rate.sampling_period(),
                package_name,
                thread_name: "fasticamera-gyro".to_owned(),
            };
            start_sensor_thread(spec, move |event| {
                let [x, y, z] = event_vector(event);
                writer.push(GyroSample {
                    time: BoottimeNanos::from_nanos(event.timestamp),
                    velocity: AngularVelocity {
                        x_rad_per_s: x,
                        y_rad_per_s: y,
                        z_rad_per_s: z,
                    },
                });
            })
        })?;
        Ok(GyroSubscription::new(ring, Box::new(lease)))
    }

    fn start_orientation(&self) -> Result<OrientationSubscription, SensorSourceError> {
        let package_name = self.package_name();
        let mut classifier = OrientationClassifier::new(self.runtime.config().orientation).map_err(|error| {
            SensorSourceError::PlatformFailure {
                sensor: ACCELEROMETER_SENSOR,
                reason: error.to_string(),
            }
        })?;
        let samples_seen = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&samples_seen);
        let events = self.runtime.events().clone();
        let rate = self.runtime.config().accelerometer_rate;
        let lease = self.start_claimed_sensor(ClaimedSensor::Orientation, ACCELEROMETER_SENSOR, || {
            let spec = SensorThreadSpec {
                sensor_name: ACCELEROMETER_SENSOR,
                sensor_type: ndk_sys::ASENSOR_TYPE_ACCELEROMETER,
                sampling_period: rate.sampling_period(),
                package_name,
                thread_name: "fasticamera-orientation".to_owned(),
            };
            start_sensor_thread(spec, move |event| {
                counter.fetch_add(1, Ordering::Relaxed);
                let [x, y, z] = event_vector(event);
                let sample = AccelerationSample {
                    x,
                    y,
                    z,
                    time: BoottimeNanos::from_nanos(event.timestamp),
                };
                if let Some(orientation) = classifier.observe(sample) {
                    // Потерю события учтёт очередь; поток датчика не ждёт.
                    events.send(ShellEvent::DeviceOrientation(orientation));
                }
            })
        })?;
        Ok(OrientationSubscription::new(samples_seen, Box::new(lease)))
    }

    fn start_gravity(&self) -> Result<GravitySubscription, SensorSourceError> {
        let package_name = self.package_name();
        let settings = self.runtime.config().gravity;
        let slot = Arc::new(GravitySlot::new());
        let mut chosen_source = GravitySource::Unspecified;
        let lease = self.start_claimed_sensor(ClaimedSensor::Gravity, GRAVITY_SENSOR, || {
            // Сначала готовый датчик гравитации платформы; нет его — акселерометр с фильтром.
            let (thread, source) = start_with_fallback(
                || {
                    start_gravity_thread(
                        GravityThreadPlan {
                            sensor_name: GRAVITY_SENSOR,
                            sensor_type: ndk_sys::ASENSOR_TYPE_GRAVITY,
                            thread_name: "fasticamera-gravity",
                        },
                        &settings,
                        package_name.clone(),
                        GravityIntake::dedicated(),
                        &slot,
                    )
                },
                || {
                    start_gravity_thread(
                        GravityThreadPlan {
                            sensor_name: ACCELEROMETER_SENSOR,
                            sensor_type: ndk_sys::ASENSOR_TYPE_ACCELEROMETER,
                            thread_name: "fasticamera-gravity-accel",
                        },
                        &settings,
                        package_name.clone(),
                        GravityIntake::filtered(settings.accelerometer_time_constant),
                        &slot,
                    )
                },
            )?;
            chosen_source = source;
            Ok(thread)
        })?;
        tracing::info!(
            target: "fasticamera::shell",
            source = ?chosen_source,
            rate_hz = settings.rate.hertz(),
            "гравитация запущена"
        );
        Ok(GravitySubscription::new(slot, Box::new(lease)).with_source(chosen_source))
    }
}

/// Какой датчик и под каким именем потока запускает [`start_gravity_thread`].
struct GravityThreadPlan {
    sensor_name: &'static str,
    sensor_type: std::ffi::c_int,
    thread_name: &'static str,
}

/// Запускает поток датчика, который пропускает показания через `intake` в ящик гравитации.
fn start_gravity_thread(
    plan: GravityThreadPlan,
    settings: &GravitySettings,
    package_name: std::ffi::CString,
    mut intake: GravityIntake,
    slot: &Arc<GravitySlot>,
) -> Result<SensorThread, SensorSourceError> {
    let writer = Arc::clone(slot);
    let sensor_name = plan.sensor_name;
    let spec = SensorThreadSpec {
        sensor_name,
        // Метки событий — CLOCK_BOOTTIME, как у гироскопа и камеры.
        sensor_type: plan.sensor_type,
        sampling_period: settings.rate.sampling_period(),
        package_name,
        thread_name: plan.thread_name.to_owned(),
    };
    start_sensor_thread(spec, move |event| {
        let time = BoottimeNanos::from_nanos(event.timestamp);
        match intake.accept(time, event_vector(event)) {
            GravityIntakeOutcome::Publish(sample) => writer.publish(sample),
            // Отброшенное не теряется молча: счётчик в intake, в лог — на степенях двойки (не заливать лог).
            GravityIntakeOutcome::RejectedNonFinite | GravityIntakeOutcome::RejectedOutOfOrder => {
                let rejected = intake.rejected_samples();
                if rejected.is_power_of_two() {
                    tracing::warn!(
                        target: "fasticamera::shell",
                        sensor = sensor_name,
                        rejected,
                        "показание гравитации отброшено"
                    );
                }
            }
        }
    })
}

impl VsyncSource for AndroidShellServices {
    /// Каждая подписка — свой поток с looper и Choreographer (см. `choreographer_thread`).
    /// Уничтожение подписки будит ожидающих, останавливает и join-ит поток.
    fn start_vsync(&self) -> Result<VsyncSubscription, VsyncSourceError> {
        let slot = Arc::new(VsyncSlot::new());
        let accounting = Arc::clone(self.runtime.accounting());
        // Сбой колбэка vsync учитывается тем же счётчиком, что и сбои JNI-колбэков оболочки.
        let thread = start_choreographer_thread(
            Arc::clone(&slot),
            Box::new(move || accounting.record_native_callback_failure()),
        )?;
        Ok(VsyncSubscription::new(slot, Box::new(thread)))
    }
}

impl ThermalSource for AndroidShellServices {
    fn current_status(&self) -> Result<ThermalStatus, ThermalSourceError> {
        match self.runtime.thermal().source() {
            Some(source) => source.current_status(),
            None => Err(ThermalSourceError::Unavailable(
                "тепловой менеджер не получен при установке оболочки".to_owned(),
            )),
        }
    }

    fn headroom(&self, forecast: HeadroomForecast) -> ThermalHeadroom {
        self.runtime
            .thermal()
            .source()
            .map_or(ThermalHeadroom::Unavailable, |source| source.headroom(forecast))
    }
}

/// Держит поток опроса батареи. Уничтожение только просит поток остановиться (не ждёт его);
/// флаг занятости освобождает сам поток при выходе (см. `power_sampler`).
struct PowerSamplingLease {
    _thread: PowerSamplerThread,
}

impl PowerSource for AndroidShellServices {
    /// Только проверки и запуск потока `fasticamera-power`: ни одного вызова платформы на
    /// потоке вызывающего. Первое показание появится в кольце после первого чтения потоком.
    fn start_sampling(&self, request: PowerSamplingRequest) -> Result<PowerSampling, PowerSourceError> {
        if self.runtime.attached_platform().is_none() {
            return Err(PowerSourceError::NotAttached);
        }
        // JavaVM известна с JNI_OnLoad; отказ здесь — нарушение порядка запуска процесса.
        let vm = jni::JavaVM::singleton().map_err(|error| PowerSourceError::Unavailable {
            reason: format!("JavaVM не известна: {error}"),
        })?;
        if !self.runtime.try_claim_sensor(ClaimedSensor::BatteryPower) {
            return Err(PowerSourceError::AlreadySampling);
        }
        // Флаг уже занят: освобождение уходит в поток (сработает при его выходе), а при ошибке
        // запуска потока — сразу, вместе с несозданным замыканием.
        let claim = {
            let runtime = Arc::clone(&self.runtime);
            ClaimRelease::new(move || runtime.release_sensor(ClaimedSensor::BatteryPower))
        };
        let ring = Arc::new(PowerSampleRing::new(request.ring_capacity));
        let thread = start_power_sampler(
            vm,
            Arc::clone(&self.runtime),
            Arc::clone(&ring),
            request.period.duration(),
            request.current_now_unit,
            claim,
        )
        .map_err(|error| PowerSourceError::PlatformCallFailed {
            reason: format!("поток опроса батареи не создан: {error}"),
        })?;
        let lease = PowerSamplingLease { _thread: thread };
        tracing::info!(
            target: "fasticamera::power",
            period_ms = request.period.duration().as_millis(),
            ring_capacity = request.ring_capacity.get(),
            "опрос батареи запущен"
        );
        Ok(PowerSampling::new(ring, Box::new(lease)))
    }
}

impl ShellDiagnostics for AndroidShellServices {
    fn counters(&self) -> ShellCounters {
        self.runtime.counters()
    }
}
