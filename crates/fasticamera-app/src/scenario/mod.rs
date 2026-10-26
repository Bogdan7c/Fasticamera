//! Smoke-сценарии: проверка приложения на телефоне по intent-параметру `scenario`.
//!
//! `scripts/device-smoke.sh <сценарий>` запускает Activity с `-e scenario <имя>`, Kotlin
//! передаёт имя сюда, а итог пишется в logcat одной строкой `FASTICAMERA_RESULT {json}`
//! (см. `result_line`). Логика сценария — только в Rust, Kotlin лишь маршрутизирует.
//!
//! Два вида сценариев:
//! * мгновенные (`hello`, `panic`) — итог сразу, в вызове `runScenario`;
//! * наблюдающие (`lifecycle`, `permission`, `motion`, `platform`, модуль [`observing`]) —
//!   передаются маршрутизатору событий и пишут итог позже, по событиям оболочки.

mod hello;
#[cfg(feature = "debug-scenarios")]
mod intentional_panic;
pub mod lifecycle;
pub mod motion;
pub mod observing;
pub mod permission;
pub mod platform;

use serde::{Deserialize, Serialize};

use crate::jni_boundary::run_guarded;
use crate::panic_report::PanicReport;
use crate::runtime_config::RuntimeConfig;

pub use hello::HelloReport;

/// Известные сценарии. Отладочные существуют только при фиче `debug-scenarios`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Библиотека загружена: версия сборки, ABI, конфиг.
    Hello,
    /// Намеренная паника: проверка, что место и сообщение доходят до logcat.
    #[cfg(feature = "debug-scenarios")]
    Panic,
    /// Свернуть и развернуть: порядок событий, утечки глобальных ссылок JNI.
    Lifecycle,
    /// Системный диалог разрешения камеры: отказ, постоянный отказ.
    Permission,
    /// Гироскоп 400 Гц и акселерометр ориентации.
    Motion,
    /// Тепло, частота кадров surface, все паттерны хаптики.
    Platform,
}

/// Как сценарий выполняется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioMode {
    /// Итог сразу, в вызове `runScenario`.
    Immediate,
    /// Итог позже: сценарий наблюдает события оболочки в маршрутизаторе.
    Observing,
}

/// Имена сценариев, которые парсер знает вообще (включая отладочные).
const DEBUG_ONLY_SCENARIO_NAMES: &[&str] = &["panic"];

impl Scenario {
    /// Все сценарии этой сборки.
    pub fn available() -> &'static [Scenario] {
        &[
            Scenario::Hello,
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic,
            Scenario::Lifecycle,
            Scenario::Permission,
            Scenario::Motion,
            Scenario::Platform,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Hello => "hello",
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic => "panic",
            Scenario::Lifecycle => "lifecycle",
            Scenario::Permission => "permission",
            Scenario::Motion => "motion",
            Scenario::Platform => "platform",
        }
    }

    pub fn mode(self) -> ScenarioMode {
        match self {
            Scenario::Hello => ScenarioMode::Immediate,
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic => ScenarioMode::Immediate,
            Scenario::Lifecycle | Scenario::Permission | Scenario::Motion | Scenario::Platform => {
                ScenarioMode::Observing
            }
        }
    }

    /// Наблюдающий сценарий для маршрутизатора (`None` — сценарий мгновенный).
    pub fn observing(self) -> Option<Box<dyn observing::ObservingScenario>> {
        match self {
            Scenario::Hello => None,
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic => None,
            Scenario::Lifecycle => Some(Box::new(lifecycle::LifecycleScenario::new())),
            Scenario::Permission => Some(Box::new(permission::PermissionScenario::new())),
            Scenario::Motion => Some(Box::new(motion::MotionScenario::new())),
            Scenario::Platform => Some(Box::new(platform::PlatformScenario::new())),
        }
    }

    /// Разбирает имя сценария. Отладочный сценарий в release — отдельная ошибка, а не
    /// «неизвестный»: так из отчёта сразу видно, что поставлена не та сборка.
    pub fn parse(name: &str) -> Result<Self, ScenarioError> {
        if let Some(scenario) = Self::available().iter().find(|scenario| scenario.name() == name) {
            return Ok(*scenario);
        }
        if DEBUG_ONLY_SCENARIO_NAMES.contains(&name) {
            return Err(ScenarioError::DebugOnly { name: name.to_owned() });
        }
        Err(ScenarioError::Unknown {
            name: name.to_owned(),
            available: Self::available().iter().map(|scenario| scenario.name()).collect(),
        })
    }
}

/// Настройки сценариев (секция `scenarios` конфига нативной части).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScenarioConfig {
    /// Сколько `motion` пишет гироскоп, мс. 3000 — как замер сессии 00 (1200 показаний).
    pub motion_duration_ms: u64,
    /// Пауза между паттернами хаптики в `platform`, мс: 700 — чтобы различить их на ощупь.
    pub haptic_interval_ms: u64,
    /// Какую частоту `platform` просит у surface, Гц: 120 — максимум экрана (сессия 00).
    pub preferred_frame_rate_hz: f32,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            motion_duration_ms: 3000,
            haptic_interval_ms: 700,
            preferred_frame_rate_hz: 120.0,
        }
    }
}

/// Неверные настройки сценариев.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScenarioConfigError {
    #[error("{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("preferred_frame_rate_hz должно быть конечным и больше нуля, а не {0}")]
    InvalidFrameRate(f32),
}

impl ScenarioConfig {
    pub fn validate(&self) -> Result<(), ScenarioConfigError> {
        if self.motion_duration_ms == 0 {
            return Err(ScenarioConfigError::Zero {
                field: "motion_duration_ms",
            });
        }
        if self.haptic_interval_ms == 0 {
            return Err(ScenarioConfigError::Zero {
                field: "haptic_interval_ms",
            });
        }
        if !(self.preferred_frame_rate_hz.is_finite() && self.preferred_frame_rate_hz > 0.0) {
            return Err(ScenarioConfigError::InvalidFrameRate(self.preferred_frame_rate_hz));
        }
        Ok(())
    }
}

/// Почему сценарий не выполнен (без паник).
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("неизвестный сценарий {name:?}; доступны: {available:?}")]
    Unknown { name: String, available: Vec<&'static str> },
    #[error("сценарий {name:?} есть только в отладочной сборке (фича debug-scenarios)")]
    DebugOnly { name: String },
    #[error("нативная часть не инициализирована: Kotlin должен сначала вызвать initialize")]
    NotInitialized,
    #[error("отчёт сценария не сериализован: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("сценарий {name:?} наблюдающий: его выполняет маршрутизатор событий, а не runScenario")]
    NotImmediate { name: &'static str },
    #[error("сценарий {name:?} не запущен: {reason}")]
    NotStarted { name: &'static str, reason: String },
}

/// Итог запуска сценария по имени.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// Мгновенный сценарий выполнен (или имя не разобралось) — итог готов.
    Finished(ScenarioReport),
    /// Наблюдающий сценарий передан маршрутизатору; итог придёт строкой результата позже.
    Started { scenario: &'static str },
}

/// Запускает сценарий: мгновенный выполняет сразу, наблюдающий отдаёт в `start_observing`.
/// Отказ запуска наблюдающего — готовый итог `failed`, чтобы скрипт не ждал до таймаута.
pub fn dispatch(
    name: &str,
    runtime: Option<&RuntimeConfig>,
    start_observing: impl FnOnce(Box<dyn observing::ObservingScenario>) -> Result<(), String>,
) -> DispatchOutcome {
    let observing = Scenario::parse(name).ok().and_then(Scenario::observing);
    let Some(scenario) = observing else {
        return DispatchOutcome::Finished(execute(name, runtime));
    };
    let scenario_name = scenario.name();
    match start_observing(scenario) {
        Ok(()) => DispatchOutcome::Started {
            scenario: scenario_name,
        },
        Err(reason) => DispatchOutcome::Finished(ScenarioReport {
            scenario: name.to_owned(),
            outcome: ScenarioOutcome::Failed {
                error: ScenarioError::NotStarted {
                    name: scenario_name,
                    reason,
                }
                .to_string(),
            },
        }),
    }
}

/// Итог сценария. Поле `status` в JSON: `passed` / `failed` / `panicked`.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ScenarioOutcome {
    Passed { report: serde_json::Value },
    Failed { error: String },
    Panicked { panic: PanicReport },
}

/// Отчёт сценария — то, что уходит в строку `FASTICAMERA_RESULT`.
#[derive(Debug, Serialize)]
pub struct ScenarioReport {
    /// Имя, которое пришло из intent (как есть, даже если сценарий неизвестен).
    pub scenario: String,
    #[serde(flatten)]
    pub outcome: ScenarioOutcome,
}

/// Выполняет сценарий по имени. Никогда не паникует наружу: любая паника — `Panicked`.
pub fn execute(name: &str, runtime: Option<&RuntimeConfig>) -> ScenarioReport {
    let outcome = match run_guarded(|| run_named(name, runtime)) {
        Ok(Ok(report)) => ScenarioOutcome::Passed { report },
        Ok(Err(error)) => ScenarioOutcome::Failed {
            error: error.to_string(),
        },
        Err(panic) => ScenarioOutcome::Panicked { panic },
    };
    ScenarioReport {
        scenario: name.to_owned(),
        outcome,
    }
}

fn run_named(name: &str, runtime: Option<&RuntimeConfig>) -> Result<serde_json::Value, ScenarioError> {
    let scenario = Scenario::parse(name)?;
    let runtime = runtime.ok_or(ScenarioError::NotInitialized)?;
    tracing::info!(target: "fasticamera::scenario", scenario = scenario.name(), "сценарий запущен");
    match scenario {
        Scenario::Hello => Ok(serde_json::to_value(HelloReport::collect(runtime))?),
        #[cfg(feature = "debug-scenarios")]
        Scenario::Panic => intentional_panic::run(),
        Scenario::Lifecycle | Scenario::Permission | Scenario::Motion | Scenario::Platform => {
            Err(ScenarioError::NotImmediate { name: scenario.name() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panic_report::install_panic_hook;

    fn debug_runtime() -> RuntimeConfig {
        RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"debug"}"#).expect("конфиг")
    }

    #[test]
    fn hello_passes_and_reports_build_and_abi() {
        let report = execute("hello", Some(&debug_runtime()));
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(json["scenario"], "hello");
        assert_eq!(json["status"], "passed");
        assert_eq!(json["report"]["build"]["crate_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            json["report"]["abi"],
            crate::build_info::android_abi_for_arch(std::env::consts::ARCH)
        );
        assert_eq!(json["report"]["build_type"], "debug");
        assert_eq!(json["report"]["log_filter"], "debug");
    }

    #[test]
    fn unknown_scenario_fails_with_available_list() {
        let json = serde_json::to_value(execute("nope", Some(&debug_runtime()))).expect("json");
        assert_eq!(json["status"], "failed");
        let error = json["error"].as_str().expect("error");
        assert!(error.contains("nope") && error.contains("hello"), "{error}");
    }

    #[test]
    fn scenario_before_initialize_fails_explicitly() {
        let json = serde_json::to_value(execute("hello", None)).expect("json");
        assert_eq!(json["status"], "failed");
        assert!(json["error"].as_str().expect("error").contains("initialize"));
    }

    #[cfg(not(feature = "debug-scenarios"))]
    #[test]
    fn panic_scenario_is_debug_only_without_feature() {
        assert!(matches!(Scenario::parse("panic"), Err(ScenarioError::DebugOnly { .. })));
    }

    #[cfg(feature = "debug-scenarios")]
    #[test]
    fn panic_scenario_reports_location_in_its_module() {
        install_panic_hook();
        let json = serde_json::to_value(execute("panic", Some(&debug_runtime()))).expect("json");
        assert_eq!(json["status"], "panicked");
        let location = json["panic"]["location"].as_str().expect("location");
        assert!(location.contains("scenario/intentional_panic.rs:"), "{location}");
        assert!(
            json["panic"]["message"]
                .as_str()
                .expect("message")
                .contains("намеренная паника")
        );
    }

    #[test]
    fn observing_scenario_is_handed_to_router_not_run_inline() {
        let mut handed_over = None;
        let outcome = dispatch("lifecycle", Some(&debug_runtime()), |scenario| {
            handed_over = Some(scenario.name());
            Ok(())
        });
        assert!(matches!(outcome, DispatchOutcome::Started { scenario: "lifecycle" }));
        assert_eq!(handed_over, Some("lifecycle"));
    }

    #[test]
    fn observing_scenario_start_failure_is_immediate_failed_report() {
        let outcome = dispatch("motion", Some(&debug_runtime()), |_| {
            Err("оболочка не установлена".to_owned())
        });
        let DispatchOutcome::Finished(report) = outcome else {
            panic!("ожидался готовый итог");
        };
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(json["scenario"], "motion");
        assert_eq!(json["status"], "failed");
        assert!(
            json["error"]
                .as_str()
                .expect("error")
                .contains("оболочка не установлена")
        );
    }

    #[test]
    fn immediate_scenario_does_not_touch_router() {
        let outcome = dispatch("hello", Some(&debug_runtime()), |_| {
            panic!("hello не должен уходить в маршрутизатор")
        });
        assert!(matches!(outcome, DispatchOutcome::Finished(report) if report.scenario == "hello"));
    }

    #[test]
    fn observing_scenario_via_execute_is_explicit_error() {
        let json = serde_json::to_value(execute("permission", Some(&debug_runtime()))).expect("json");
        assert_eq!(json["status"], "failed");
        assert!(json["error"].as_str().expect("error").contains("маршрутизатор"));
    }

    #[test]
    fn scenario_config_rejects_zero_and_bad_rate() {
        assert!(ScenarioConfig::default().validate().is_ok());
        let zero = ScenarioConfig {
            motion_duration_ms: 0,
            ..ScenarioConfig::default()
        };
        assert!(matches!(zero.validate(), Err(ScenarioConfigError::Zero { .. })));
        let rate = ScenarioConfig {
            preferred_frame_rate_hz: f32::NAN,
            ..ScenarioConfig::default()
        };
        assert!(matches!(rate.validate(), Err(ScenarioConfigError::InvalidFrameRate(_))));
    }

    #[test]
    fn hook_install_is_idempotent() {
        install_panic_hook();
        install_panic_hook();
        let json = serde_json::to_value(execute("hello", Some(&debug_runtime()))).expect("json");
        assert_eq!(json["status"], "passed");
    }
}
