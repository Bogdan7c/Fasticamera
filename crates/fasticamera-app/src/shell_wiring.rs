//! Сборка оболочки и маршрутизатора (только Android).
//!
//! Composition root: ставит `android-shell`, раздаёт его сервисы через нейтральные трейты
//! запускает поток рендера (решение 31: маршрутизатор отдаёт ему surface), поток камеры и поток маршрутизатора.
//! Решений фич здесь нет.
//!
//! Схема потока кадров (сессия 05): ОДИН `RawFrameStream` создаётся до рендера и камеры.
//! ```text
//! camera-android ──producer()──▶ RawFrameStream ──viewfinder()──▶ render-loop (камера на экране)
//!                                      │
//!                                      └── RawStreamServices ──▶ сценарии (счётчики, проба CPU, часы)
//! ```
//! Рендер на Android всегда в режиме показа камеры: пока потока нет, он рисует чёрное.
//!
//! Если GPU или vsync не поднялись, приложение не падает и не зависает: рендер помечается
//! недоступным (причина видна сценариям), а маршрутизатор подтверждает освобождение surface сразу.
//! Если не поднялась камера, приложение тоже живёт: сервис камеры хранит причину (`CameraServices::unavailable`).

use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use android_shell::android::{AndroidPerformanceHints, AndroidShellServices, ShellInstallError, ThermalListenerState};
use camera_contract::lens_shading::LensShadingCalibrations;
use gpu_vulkan::validation::ValidationMode;
use look_contract::LookRecipe;
use raw_frame_stream::RawFrameStream;
use render_loop::config::RenderLoopConfig;
use render_loop::thread::{RenderLoopParts, VsyncFrameClock};
use render_loop::viewfinder::AutoExposureLinks;
use render_loop::vulkan_backend::{VulkanBackendConfig, VulkanDisplayBackend};
use shell_contract::text_input::TextInputService;
use shell_contract::thermal_policy::{ThermalPolicy, ThermalPolicyConfigError};
use shell_contract::vsync::VsyncSource;

use android_shell::package_name::PackageName;

use crate::app_directory::AppDirectory;
use crate::auto_exposure_link::RenderAutoExposure;
use crate::camera_config::CameraConfig;
use crate::camera_service::CameraServices;
use crate::camera_wiring::{CameraBackendParts, CameraLinks, RenderLinks, start_camera_with_links};
use crate::event_router::{EventRouter, LogcatScenarioOutput, RouterParts, ScenarioStarter};
use crate::flash_still_relay::FlashStillRelay;
use crate::gpu_services::{GpuDiagnostics, GpuServices, ValidationState};
use crate::jni_boundary::run_guarded;
use crate::lens_shading_profile::{LensShadingProfileError, embedded_lens_shading};
use crate::look_startup::{catalog_environment, open_look_service};
use crate::pipeline_cache_file::PipelineCacheFile;
use crate::raw_stream_wiring::{RawStreamCreation, create_raw_stream, observe_stream};
use crate::render_wiring::RenderRuntime;
use crate::runtime_config::RuntimeConfig;
use crate::saturation_bench::services::SaturationServices;
use crate::scenario::observing::{ObservingScenario, PlatformFacts, ShellServices, ThermalListenerReport};
use crate::still_android_wiring::assemble_still;
use crate::text_input_broker::TextInputBroker;
use crate::vulkan_diagnostics::VulkanGpuDiagnostics;
use crate::white_memory::{WhiteMemoryHandoff, open_white_memory};
use crate::white_sources::product_white_sources;
use render_loop::pipeline_cache_store::SharedPipelineCacheStore;

/// Что остаётся у composition root после сборки: вход для наблюдающих сценариев.
struct ShellWiring {
    starter: ScenarioStarter,
    /// Служба записи снимков живёт столько же, сколько процесс: она лежит в статике `SHELL_WIRING`, а `Drop`
    /// статика в Rust не вызывается НИКОГДА, поэтому корректной остановки с ожиданием рабочего потока здесь нет
    /// и обещать её нельзя. Точки остановки платформы тоже нет: `MainActivity.onDestroy` отключает только
    /// платформу, а Activity пересоздаётся при смене конфигурации, и служба обязана пережить это (рендер и
    /// камера живут так же). Что бывает при смерти процесса: рабочий поток обрывается вместе с ним, черновики
    /// (`IS_PENDING = 1`) остаются в хранилище, а незавершённые снимки итога не получают — их убирает уборка
    /// сирот при следующем старте (`still_orphan_sweep`). `None` — работает запасной фейк.
    /// Под `Mutex` ради `Sync` статика (внутри приёмник канала); замок не берётся: служба только держится живой.
    _still_service: Mutex<Option<photo_output::service::StillService>>,
}

static SHELL_WIRING: OnceLock<ShellWiring> = OnceLock::new();

/// Почему оболочка не собрана.
#[derive(Debug, thiserror::Error)]
pub enum ShellWiringError {
    #[error("{0}")]
    Install(#[from] ShellInstallError),
    #[error("тепловая политика: {0}")]
    ThermalPolicy(#[from] ThermalPolicyConfigError),
    #[error("поток маршрутизатора не создан: {0}")]
    RouterThread(std::io::Error),
    #[error("профиль калибровки виньетирования: {0}")]
    LensShadingProfile(#[from] LensShadingProfileError),
    /// Конфиг хранилища пресетов вида не годится (его проверяет и `RuntimeConfig::from_json`: сюда доходит только в обход).
    #[error("хранилище пресетов вида: {0}")]
    LookPresets(#[from] look_presets::ConfigError),
}

/// Ставит оболочку один раз на процесс; повторный вызов — no-op. `package_name` — имя пакета процесса
/// (решение 106): по нему оболочка включает датчики, не дожидаясь подключения Activity. `app_directory` — папка
/// приложения (`filesDir`): там лежит файл пресетов вида (решение 99).
pub fn ensure_installed(
    config: &RuntimeConfig,
    package_name: PackageName,
    app_directory: &AppDirectory,
) -> Result<(), ShellWiringError> {
    if SHELL_WIRING.get().is_some() {
        return Ok(());
    }
    // Пресеты вида открываются ДО рендера: начальный вид (активный пресет) обязан действовать с первого кадра
    // видоискателя, поэтому уходит бэкенду при сборке, а не командой после старта.
    let look = open_look_service(
        app_directory,
        config.look_presets(),
        &catalog_environment(config.render()),
    )?;
    let thermal_policy = Arc::new(ThermalPolicy::new(config.thermal_policy())?);
    let handles = android_shell::android::install(config.shell(), package_name)?;
    let services = handles.services;
    let validation = config.validation_mode();
    // Поток кадров один на процесс: его концы получают и рендер (видоискатель), и камера (производитель).
    let raw_stream = create_raw_stream(&config.camera().raw_stream);
    // Калибровка виньетирования вшита в сборку и сверяется с телефоном (`device.make/model`): битый профиль
    // останавливает запуск явной ошибкой, чужой телефон — пустой набор с записью в лог.
    let lens_shading = embedded_lens_shading(config.device())?;
    let lens_shading_for_saturation = lens_shading.clone();
    // Ящики связи камеры с рендером один набор на процесс: AWB и наш AE. Камера получает свою половину,
    // рендер — свою (клоны делят состояние).
    // Память белой открывается ДО движка: стартовая белая (запомненная или запасной свет) известна при его сборке.
    let white_memory = open_white_memory(app_directory, &config.camera().white_memory);
    let mut links = CameraLinks {
        white_memory: WhiteMemoryHandoff::holding(white_memory),
        // Ёмкость памяти итогов вспышки — та же, что у журнала снимков (конфиг `photo.journal_capacity`).
        flash_relay: Arc::new(FlashStillRelay::new(config.photo().journal_capacity)),
        ..CameraLinks::default()
    };
    // Реестр источников белого собирается здесь, из конфига рендера: рендеру и движку источники не вшиты.
    let mut render_links = links.render_side(config.camera().exposure_controller);
    render_links.white.sources = product_white_sources(config.render());
    let (render, render_auto_exposure) = start_render(
        config.render(),
        validation.clone(),
        &services,
        &raw_stream.stream,
        render_links,
        // Стартовый рецепт — основа и ручки активного пресета из файла (решение 195): основа тоже действует с
        // первого кадра, конфиг `viewfinder.tone.base` лишь умолчание встроенных пресетов библиотеки.
        look.handle.active_recipe(),
        // Кэш пайплайнов GPU между запусками: файл в той же папке приложения.
        Arc::new(PipelineCacheFile::in_app_directory(app_directory)),
    );
    // Камера узнаёт, есть ли у нашего AE рендер: без него она получит вендорский AE (с предупреждением).
    links.render_auto_exposure = render_auto_exposure;
    // Один набор диагностик на процесс: стенд насыщения (16a) берёт его долгоживущий контекст GPU.
    let diagnostics = Arc::new(VulkanGpuDiagnostics::new(validation));
    let gpu = GpuServices::new(
        render.status,
        Arc::clone(&diagnostics) as Arc<dyn GpuDiagnostics>,
        config.render().clone(),
        render.control,
    )
    .with_saturation(SaturationServices::new(diagnostics, lens_shading_for_saturation));
    let flash_relay = Arc::clone(&links.flash_relay);
    let camera = start_android_camera(config.camera(), lens_shading, &raw_stream, links);
    // Снимок: настоящая служба, если есть рендер, камера и паспорт телефона; иначе явный запасной фейк.
    let still = assemble_still(config, &services, render.still, &camera, flash_relay);
    let (router, starter) = EventRouter::new(RouterParts {
        events: handles.events,
        services: shell_services(&services),
        thermal_policy,
        scenario_config: *config.scenarios(),
        gyro_request: services.default_gyro_request(),
        output: Box::new(LogcatScenarioOutput::default()),
        route: render.route,
        gpu,
        camera,
        camera_screen: config.camera_screen().clone(),
        capture: still.capture,
        look: Some(look.handle),
    });
    // Диалог ввода текста (решение 197): брокер ведёт запросы и ответы. Приёмник итогов подключит экран камеры
    // (`TextInputBroker::set_sink`), пока его нет — ответы считаются и логируются.
    let text_input = TextInputBroker::new(Arc::clone(&services) as Arc<dyn TextInputService>);
    let router = router.with_text_input(text_input);
    thread::Builder::new()
        .name("fasticamera-router".to_owned())
        .spawn(move || {
            // Сценарии изолированы внутри маршрутизатора; сюда долетает только паника его
            // собственного кода — громко, потому что без маршрутизатора события никто не читает.
            if let Err(panic) = run_guarded(|| router.run()) {
                tracing::error!(target: "fasticamera::router", panic = %panic.log_text(), "маршрутизатор событий упал");
            }
        })
        .map_err(ShellWiringError::RouterThread)?;
    // Гонки нет: initialize вызывается на главном потоке Android.
    // Второй `set` означает, что два вызова прошли проверку `get()` одновременно: оболочка и поток
    // маршрутизатора созданы дважды. Молча отбрасывать лишнюю обвязку нельзя — это нарушение
    // инварианта «один раз на процесс», о нём громко сообщаем.
    if SHELL_WIRING
        .set(ShellWiring {
            starter,
            _still_service: Mutex::new(still.service),
        })
        .is_err()
    {
        tracing::error!(
            target: "fasticamera::shell_wiring",
            "оболочка уже установлена: повторная обвязка отброшена (initialize вызван параллельно?)"
        );
    }
    Ok(())
}

/// Поднимает GPU, vsync и поток рендера. Любой отказ — `RenderRuntime::unavailable` с причиной.
fn start_render(
    render_config: &RenderLoopConfig,
    validation: ValidationMode,
    services: &Arc<AndroidShellServices>,
    raw_stream: &RawFrameStream,
    links: RenderLinks,
    initial_look: LookRecipe,
    pipeline_cache: SharedPipelineCacheStore,
) -> (RenderRuntime, RenderAutoExposure) {
    match try_start_render(
        render_config,
        validation,
        services,
        raw_stream,
        links,
        initial_look,
        pipeline_cache,
    ) {
        Ok(runtime) => (runtime, RenderAutoExposure::Connected),
        Err(reason) => {
            let unavailable = RenderAutoExposure::Unavailable { reason: reason.clone() };
            (RenderRuntime::unavailable(reason), unavailable)
        }
    }
}

fn try_start_render(
    render_config: &RenderLoopConfig,
    validation: ValidationMode,
    services: &Arc<AndroidShellServices>,
    raw_stream: &RawFrameStream,
    links: RenderLinks,
    initial_look: LookRecipe,
    pipeline_cache: SharedPipelineCacheStore,
) -> Result<RenderRuntime, String> {
    let RenderLinks { white, auto_exposure } = links;
    let backend_config = VulkanBackendConfig::from_render_config(render_config, validation.clone())
        .map_err(|error| format!("настройки бэкенда рендера: {error}"))?
        .with_pipeline_cache_store(pipeline_cache);
    // Режим показа камеры всегда: видоискатель отдаёт кадры, пока камера не стримит — рисуется чёрное.
    let backend = VulkanDisplayBackend::new_android_camera_preview(backend_config, raw_stream.viewfinder(), white)
        .map_err(|error| format!("GPU не поднялся: {error}"))?
        // Стартовый вид — активный пресет из файла: ручки и основа действуют с первого кадра (решения 99, 195).
        .with_initial_look(Arc::new(initial_look.settings))
        .with_initial_tone_base(initial_look.base);
    // Наш AE (08a): рендер меряет сцену и пишет предложения движку, читает его указания. `None` —
    // у камеры вендорский AE (`camera.exposure_controller = "vendor"`, A/B): рендеру мерить нечего,
    // подтяжка ISP остаётся единичной.
    // Снимки без задержки затвора (сессия 14): рендер берёт кадры из кольца снимков того же потока камеры.
    let backend = backend.with_still_capture(raw_stream.snapshot_ring());
    let backend = match auto_exposure {
        Some(render_end) => backend.with_auto_exposure(AutoExposureLinks {
            directives: render_end.directives,
            proposals: render_end.proposals,
        }),
        None => backend,
    };
    let subscription = services
        .start_vsync()
        .map_err(|error| format!("vsync не запущен: {error}"))?;
    let parts = RenderLoopParts {
        config: render_config.clone(),
        backend,
        clock: VsyncFrameClock::new(subscription),
        commands: services.clone(),
        // ADPF: поток рендера сам создаст сессию на себя, если `render.adpf.mode = enabled`
        // (по умолчанию выключено) или его включит команда сценария.
        performance_hints: Arc::new(AndroidPerformanceHints::new()),
    };
    RenderRuntime::spawn(parts, ValidationState::of(&validation))
        .map_err(|error| format!("поток рендера не запущен: {error}"))
}

/// Поднимает поток камеры на бэкенде Camera2 NDK. Менеджер Camera2 создаётся уже на потоке камеры;
/// отказ запуска — сервис «камера недоступна: причина» (приложение живёт дальше).
fn start_android_camera(
    config: &CameraConfig,
    lens_shading: LensShadingCalibrations,
    raw_stream: &RawStreamCreation,
    links: CameraLinks,
) -> CameraServices {
    let parts = camera_android::backend_factory(config.android.clone(), lens_shading, raw_stream.stream.producer());
    start_camera_with_links(
        config,
        CameraBackendParts {
            factory: parts.factory,
            diagnostics: parts.diagnostics,
            raw_stream: observe_stream(raw_stream, parts.cpu_access_probe),
        },
        links,
    )
}

/// Передаёт наблюдающий сценарий маршрутизатору.
pub fn start_observing(scenario: Box<dyn ObservingScenario>) -> Result<(), String> {
    match SHELL_WIRING.get() {
        Some(wiring) => wiring.starter.start(scenario),
        None => Err("оболочка не установлена (initialize не прошёл)".to_owned()),
    }
}

/// Один объект оболочки под всеми нейтральными трейтами.
fn shell_services(services: &Arc<AndroidShellServices>) -> ShellServices {
    ShellServices {
        commands: services.clone(),
        haptics: services.clone(),
        operator_prompt: services.clone(),
        motion: services.clone(),
        thermal: services.clone(),
        diagnostics: services.clone(),
        vsync: services.clone(),
        facts: Arc::new(AndroidFacts(Arc::clone(services))),
        power: services.clone(),
    }
}

/// Факты Android-оболочки для отчётов сценариев.
struct AndroidFacts(Arc<AndroidShellServices>);

impl PlatformFacts for AndroidFacts {
    fn thermal_listener(&self) -> ThermalListenerReport {
        match self.0.thermal_listener_state() {
            ThermalListenerState::Registered => ThermalListenerReport::Registered,
            ThermalListenerState::RegistrationFailed { status } => ThermalListenerReport::Failed {
                reason: format!("регистрация отвергнута, код {status}"),
            },
            ThermalListenerState::ManagerUnavailable { reason } => ThermalListenerReport::Failed { reason },
            ThermalListenerState::MonitorThreadFailed { reason } => ThermalListenerReport::Failed {
                reason: format!("поток монитора не создан: {reason}"),
            },
        }
    }

    fn has_vibrator(&self) -> bool {
        self.0
            .haptic_capabilities()
            .is_some_and(|capabilities| capabilities.has_vibrator)
    }
}
