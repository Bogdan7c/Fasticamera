//! Сборка оболочки и маршрутизатора (только Android).
//!
//! Composition root: ставит `android-shell`, раздаёт его сервисы через нейтральные трейты
//! запускает поток рендера (решение 31: маршрутизатор отдаёт ему surface), поток камеры и поток маршрутизатора.
//! Решений фич здесь нет.
//!
//! Если GPU или vsync не поднялись, приложение не падает и не зависает: рендер помечается
//! недоступным (причина видна сценариям), а маршрутизатор подтверждает освобождение surface сразу.
//! Если не поднялась камера, приложение тоже живёт: сервис камеры хранит причину (`CameraServices::unavailable`).

use std::sync::{Arc, OnceLock};
use std::thread;

use android_shell::android::{AndroidShellServices, ShellInstallError, ThermalListenerState};
use gpu_vulkan::validation::ValidationMode;
use render_loop::config::RenderLoopConfig;
use render_loop::thread::{RenderLoopParts, VsyncFrameClock};
use render_loop::vulkan_backend::{VulkanBackendConfig, VulkanDisplayBackend};
use shell_contract::thermal_policy::{ThermalPolicy, ThermalPolicyConfigError};
use shell_contract::vsync::VsyncSource;

use crate::camera_config::CameraConfig;
use crate::camera_service::CameraServices;
use crate::camera_wiring::{CameraBackendParts, start_camera};
use crate::event_router::{EventRouter, LogcatScenarioOutput, RouterParts, ScenarioStarter};
use crate::gpu_services::{GpuServices, ValidationState};
use crate::jni_boundary::run_guarded;
use crate::render_wiring::RenderRuntime;
use crate::runtime_config::RuntimeConfig;
use crate::scenario::observing::{ObservingScenario, PlatformFacts, ShellServices, ThermalListenerReport};
use crate::vulkan_diagnostics::VulkanGpuDiagnostics;

/// Что остаётся у composition root после сборки: вход для наблюдающих сценариев.
struct ShellWiring {
    starter: ScenarioStarter,
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
}

/// Ставит оболочку один раз на процесс; повторный вызов — no-op.
pub fn ensure_installed(config: &RuntimeConfig) -> Result<(), ShellWiringError> {
    if SHELL_WIRING.get().is_some() {
        return Ok(());
    }
    let thermal_policy = Arc::new(ThermalPolicy::new(config.thermal_policy())?);
    let handles = android_shell::android::install(config.shell())?;
    let services = handles.services;
    let validation = config.validation_mode();
    let render = start_render(config.render(), validation.clone(), &services);
    let gpu = GpuServices {
        render: render.status,
        diagnostics: Arc::new(VulkanGpuDiagnostics::new(validation)),
        render_config: config.render().clone(),
    };
    let camera = start_android_camera(config.camera());
    let (router, starter) = EventRouter::new(RouterParts {
        events: handles.events,
        services: shell_services(&services),
        thermal_policy,
        scenario_config: *config.scenarios(),
        gyro_request: services.default_gyro_request(),
        output: Box::new(LogcatScenarioOutput),
        route: render.route,
        gpu,
        camera,
    });
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
    if SHELL_WIRING.set(ShellWiring { starter }).is_err() {
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
) -> RenderRuntime {
    match try_start_render(render_config, validation, services) {
        Ok(runtime) => runtime,
        Err(reason) => RenderRuntime::unavailable(reason),
    }
}

fn try_start_render(
    render_config: &RenderLoopConfig,
    validation: ValidationMode,
    services: &Arc<AndroidShellServices>,
) -> Result<RenderRuntime, String> {
    let backend_config = VulkanBackendConfig::from_render_config(render_config, validation.clone())
        .map_err(|error| format!("настройки бэкенда рендера: {error}"))?;
    let backend =
        VulkanDisplayBackend::new_android(backend_config).map_err(|error| format!("GPU не поднялся: {error}"))?;
    let subscription = services
        .start_vsync()
        .map_err(|error| format!("vsync не запущен: {error}"))?;
    let parts = RenderLoopParts {
        config: render_config.clone(),
        backend,
        clock: VsyncFrameClock::new(subscription),
        commands: services.clone(),
    };
    RenderRuntime::spawn(parts, ValidationState::of(&validation))
        .map_err(|error| format!("поток рендера не запущен: {error}"))
}

/// Поднимает поток камеры на бэкенде Camera2 NDK. Менеджер Camera2 создаётся уже на потоке камеры;
/// отказ запуска — сервис «камера недоступна: причина» (приложение живёт дальше).
fn start_android_camera(config: &CameraConfig) -> CameraServices {
    let parts = camera_android::backend_factory(config.android.clone());
    start_camera(
        config,
        CameraBackendParts {
            factory: parts.factory,
            diagnostics: parts.diagnostics,
        },
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
        motion: services.clone(),
        thermal: services.clone(),
        diagnostics: services.clone(),
        vsync: services.clone(),
        facts: Arc::new(AndroidFacts(Arc::clone(services))),
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
