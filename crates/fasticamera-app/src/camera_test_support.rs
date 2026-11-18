//! Подставные объекты для тестов сервиса камеры, маршрутизатора и сценариев камеры. Только для тестов.
//!
//! Движок камеры в тестах настоящий (поток `fasticamera-camera`), подменён только бэкенд платформы:
//! `FakeBackendControl` из `camera-core` плюс обёртка, которая отдаёт рычаг частоты так, как это
//! делает Camera2-бэкенд на телефоне (fake сам всегда отвечает `HalDefault`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use camera_android::companion_stream::CompanionStreamStatus;
use camera_android::{BackendDiagnostics, CpuAccessProbe};
use camera_contract::capabilities::{CameraDescriptor, FpsRange};
use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, FrameRateMechanism, ReadoutAspect, ResolvedStream};
use camera_contract::three_a::{BurstId, ExposureBracket, PointRequestId};
use camera_core::backend::{
    BackendEventSink, BackendFactory, BackendFailure, CameraBackend, ConfigureFailure, DeviceGeneration, OpenFailure,
    RequestFailure, SessionGeneration,
};
use camera_core::fake::FakeBackendControl;
use camera_core::request_controls::{AfTrigger, RequestControls};
use camera_core::{CameraHandle, CameraState, CameraStatus};
use raw_frame_stream::RawStreamCounters;

use crate::camera_config::CameraConfig;
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::camera_service::{BackendDiagnosticsSource, CameraServices};
use crate::camera_wiring::{CameraBackendParts, start_camera};
use crate::clock_offset::SystemClocks;
use crate::raw_stream_services::{RawStreamCounterSource, RawStreamServices};

/// Состояние вспомогательного потока 3A «здорового» телефона (размер зонда 08).
const ACTIVE_COMPANION: CompanionStreamStatus = CompanionStreamStatus::Active {
    width: 640,
    height: 480,
};

/// Сколько ждём в тестах состояния движка: запас на загруженную машину, ничего не висит вечно.
pub const WAIT: Duration = Duration::from_secs(10);

/// Какой рычаг частоты отдаёт «телефон» для запрошенного потока (политика реального бэкенда:
/// основная «2» 16:9@60 — публичный диапазон, 4:3@60 — вендорский, остальное @60 — публичный).
pub fn phone_like_mechanism(stream: &ResolvedStream) -> FrameRateMechanism {
    let rate = stream.frame_rate.frames_per_second();
    if rate < 60 {
        return FrameRateMechanism::HalDefault;
    }
    let is_main = stream.camera.as_str() == "2";
    if is_main && stream.readout == ReadoutAspect::FourByThree {
        FrameRateMechanism::VendorSensorFrameDurationOverride {
            frame_duration: stream.frame_rate.frame_duration(),
        }
    } else {
        FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: rate, max: rate },
        }
    }
}

/// Бэкенд-обёртка: всё делегирует fake, но в `ConfiguredStream` подставляет рычаг частоты.
struct LeverReportingBackend {
    inner: Box<dyn CameraBackend>,
    /// Пауза внутри `close_device`, мс: поток движка «занят» закрытием, а опубликованный снимок ещё
    /// показывает прежнее состояние (так выглядит медленное закрытие камеры на телефоне).
    close_device_delay_ms: Arc<AtomicU64>,
    /// Рубильник пробы CPU и журнал «была ли проба взведена в момент каждой конфигурации сессии».
    cpu_probe: CpuAccessProbe,
    probe_at_configure: Arc<Mutex<Vec<bool>>>,
}

impl CameraBackend for LeverReportingBackend {
    fn load_catalog(&mut self) -> Result<Vec<CameraDescriptor>, BackendFailure> {
        self.inner.load_catalog()
    }

    fn open_device(&mut self, camera: &CameraId, generation: DeviceGeneration) -> Result<(), OpenFailure> {
        self.inner.open_device(camera, generation)
    }

    fn configure_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        self.probe_at_configure
            .lock()
            .expect("замок")
            .push(self.cpu_probe.is_armed());
        let configured = self.inner.configure_session(stream, generation)?;
        Ok(ConfiguredStream {
            frame_rate_mechanism: phone_like_mechanism(&configured.stream),
            stream: configured.stream,
        })
    }

    fn set_repeating_request(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure> {
        self.inner.set_repeating_request(intent, controls)
    }

    fn submit_focus_trigger(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        trigger: AfTrigger,
        request: PointRequestId,
    ) -> Result<(), RequestFailure> {
        self.inner.submit_focus_trigger(intent, controls, trigger, request)
    }

    fn submit_bracket(
        &mut self,
        intent: CaptureIntent,
        base: &RequestControls,
        bracket: &ExposureBracket,
        burst: BurstId,
    ) -> Result<(), RequestFailure> {
        self.inner.submit_bracket(intent, base, bracket, burst)
    }

    fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        self.inner.stop_repeating()
    }

    fn close_session(&mut self) {
        self.inner.close_session();
    }

    fn close_device(&mut self) {
        let delay_ms = self.close_device_delay_ms.load(Ordering::SeqCst);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        self.inner.close_device();
    }
}

fn lever_reporting_factory(
    control: &Arc<FakeBackendControl>,
    close_device_delay_ms: Arc<AtomicU64>,
    cpu_probe: CpuAccessProbe,
    probe_at_configure: Arc<Mutex<Vec<bool>>>,
) -> BackendFactory {
    let inner_factory = control.factory();
    Box::new(move |sink: BackendEventSink| {
        let inner = inner_factory(sink)?;
        Ok(Box::new(LeverReportingBackend {
            inner,
            close_device_delay_ms,
            cpu_probe,
            probe_at_configure,
        }) as Box<dyn CameraBackend>)
    })
}

/// Счётчики потока кадров, которые тест выставляет вручную («поток» в тестах не настоящий).
#[derive(Default)]
pub struct ScriptedStreamCounters(Mutex<RawStreamCounters>);

impl ScriptedStreamCounters {
    /// Меняет счётчики под замком.
    pub fn update(&self, change: impl FnOnce(&mut RawStreamCounters)) {
        change(&mut self.0.lock().expect("замок"));
    }
}

impl RawStreamCounterSource for ScriptedStreamCounters {
    fn counters(&self) -> RawStreamCounters {
        *self.0.lock().expect("замок")
    }
}

/// Движок камеры на fake-бэкенде вместе со всем, чем тест им управляет.
pub struct FakeCameraRig {
    pub control: Arc<FakeBackendControl>,
    /// Диагностика «бэкенда»: тест сам заполняет счётчики RAW-приёмника.
    pub diagnostics: Arc<BackendDiagnostics>,
    pub services: CameraServices,
    /// Счётчики потока кадров «бэкенда»: тест заполняет их сам.
    pub stream_counters: Arc<ScriptedStreamCounters>,
    /// Рубильник пробы CPU-доступа, общий с сервисом (как у настоящего бэкенда).
    pub cpu_probe: CpuAccessProbe,
    /// Была ли проба взведена в момент каждой конфигурации сессии (читатель выбирает usage при создании).
    pub probe_at_configure: Arc<Mutex<Vec<bool>>>,
    /// Пауза внутри закрытия устройства, мс (0 — без паузы); тест меняет её на ходу.
    pub close_device_delay_ms: Arc<AtomicU64>,
}

impl FakeCameraRig {
    /// Движок с настройками по умолчанию и каталогом OnePlus 11.
    pub fn new() -> Self {
        Self::with_config(CameraConfig::default())
    }

    /// Движок с заданными параметрами сценариев (остальное — по умолчанию).
    pub fn with_scenarios(scenarios: CameraScenarioConfig) -> Self {
        Self::with_config(CameraConfig {
            scenarios,
            ..CameraConfig::default()
        })
    }

    /// Движок с заданными настройками, но БЕЗ автозапуска видоискателя: тесты сценариев и
    /// жизненного цикла начинают с закрытой камеры и сами решают, что открыть. Автозапуск
    /// проверяют через [`FakeCameraRig::with_product_config`].
    pub fn with_config(mut config: CameraConfig) -> Self {
        config.viewfinder_autostart.enabled = false;
        Self::with_product_config(config)
    }

    /// Движок с настройками как у продукта, включая `camera.viewfinder_autostart` как есть.
    pub fn with_product_config(config: CameraConfig) -> Self {
        let control = FakeBackendControl::new();
        let diagnostics = Arc::new(BackendDiagnostics::new(config.android.failure_history_capacity));
        let close_device_delay_ms = Arc::new(AtomicU64::new(0));
        // Как у настоящего бэкенда после конфигурации сессии: вспомогательный поток 3A в сессии.
        diagnostics.record_companion_status(ACTIVE_COMPANION);
        let stream_counters = Arc::new(ScriptedStreamCounters::default());
        let cpu_probe = CpuAccessProbe::disarmed();
        let probe_at_configure = Arc::new(Mutex::new(Vec::new()));
        let services = start_camera(
            &config,
            CameraBackendParts {
                factory: lever_reporting_factory(
                    &control,
                    Arc::clone(&close_device_delay_ms),
                    cpu_probe.clone(),
                    Arc::clone(&probe_at_configure),
                ),
                diagnostics: Arc::clone(&diagnostics) as Arc<dyn BackendDiagnosticsSource>,
                raw_stream: RawStreamServices::new(
                    Arc::clone(&stream_counters) as Arc<dyn RawStreamCounterSource>,
                    cpu_probe.clone(),
                    Arc::new(SystemClocks),
                    config.raw_stream.clone(),
                ),
            },
        );
        Self {
            control,
            diagnostics,
            services,
            stream_counters,
            cpu_probe,
            probe_at_configure,
            close_device_delay_ms,
        }
    }

    /// Ручка движка (сервис запущен всегда, иначе тест неверно собран).
    pub fn handle(&self) -> CameraHandle {
        self.services.running().expect("движок камеры запущен").handle().clone()
    }

    /// Ждёт состояния движка по имени.
    pub fn wait_state(&self, name: &'static str) -> CameraStatus {
        let handle = self.handle();
        handle
            .wait_until_status(WAIT, |status| status.state.name() == name)
            .unwrap_or_else(|| panic!("состояние {name} не наступило, сейчас {}", handle.status().state.name()))
    }

    /// Ждёт, пока движок покажет `Streaming`.
    pub fn wait_streaming(&self) -> CameraStatus {
        self.wait_state("streaming")
    }

    pub fn state(&self) -> CameraState {
        self.handle().status().state
    }
}

/// Отпускает ли подставной слушатель принятые кадры.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FramesAreReleased {
    Yes,
    No,
}

/// Поток, который «снимает» кадры: шлёт результаты в fake и ведёт счётчики RAW-приёмника,
/// пока жив. Метки сенсора синтетические, поэтому измеренная частота = `1 / interval` независимо
/// от скорости теста.
pub struct FrameFeeder {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl FrameFeeder {
    /// Запускает подачу кадров с интервалом меток `sensor_interval`; реальная пауза между
    /// пачками — 1 мс (меток это не касается).
    pub fn start(rig: &FakeCameraRig, sensor_interval: Duration) -> Self {
        Self::spawn(
            rig,
            Some(sensor_interval),
            FramesAreReleased::Yes,
            Duration::from_millis(1),
        )
    }

    /// То же, но кадры RAW-приёмника не отпускаются (имитация утечки буферов).
    pub fn start_leaking(rig: &FakeCameraRig, sensor_interval: Duration) -> Self {
        Self::spawn(
            rig,
            Some(sensor_interval),
            FramesAreReleased::No,
            Duration::from_millis(1),
        )
    }

    /// Камера логического типа: RAW-кадры идут с реальным темпом `real_pause`, а финальных результатов
    /// HAL не шлёт вовсе (только частичные, которые движок не видит).
    pub fn start_raw_only(rig: &FakeCameraRig, real_pause: Duration) -> Self {
        Self::spawn(rig, None, FramesAreReleased::Yes, real_pause)
    }

    fn spawn(
        rig: &FakeCameraRig,
        sensor_interval: Option<Duration>,
        release: FramesAreReleased,
        real_pause: Duration,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let control = Arc::clone(&rig.control);
        let diagnostics = Arc::clone(&rig.diagnostics);
        let thread_stop = Arc::clone(&stop);
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                if control.is_repeating() {
                    if let Some(interval) = sensor_interval {
                        control.complete_frames(1, interval);
                    }
                    diagnostics.reader().record_frame_acquired();
                    if release == FramesAreReleased::Yes {
                        diagnostics.reader().record_image_returned();
                    }
                }
                thread::sleep(real_pause);
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for FrameFeeder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
