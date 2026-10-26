//! Сборка оболочки и маршрутизатора (только Android).
//!
//! Composition root: ставит `android-shell`, раздаёт его сервисы через нейтральные трейты
//! и запускает поток маршрутизатора. Решений фич здесь нет.

use std::sync::{Arc, OnceLock};
use std::thread;

use android_shell::android::{AndroidShellServices, ShellInstallError, ThermalListenerState};
use shell_contract::thermal_policy::{ThermalPolicy, ThermalPolicyConfigError};

use crate::event_router::{EventRouter, LogcatScenarioOutput, RouterParts, ScenarioStarter};
use crate::jni_boundary::run_guarded;
use crate::runtime_config::RuntimeConfig;
use crate::scenario::observing::{ObservingScenario, PlatformFacts, ShellServices, ThermalListenerReport};

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
    let (router, starter) = EventRouter::new(RouterParts {
        events: handles.events,
        services: shell_services(&services),
        thermal_policy,
        scenario_config: *config.scenarios(),
        gyro_request: services.default_gyro_request(),
        output: Box::new(LogcatScenarioOutput),
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
    let _ = SHELL_WIRING.set(ShellWiring { starter });
    Ok(())
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
