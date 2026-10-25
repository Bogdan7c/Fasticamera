//! Smoke-сценарии: проверка приложения на телефоне по intent-параметру `scenario`.
//!
//! `scripts/device-smoke.sh <сценарий>` запускает Activity с `-e scenario <имя>`, Kotlin
//! передаёт имя сюда, а итог пишется в logcat одной строкой `FASTICAMERA_RESULT {json}`
//! (см. `result_line`). Логика сценария — только в Rust, Kotlin лишь маршрутизирует.

mod hello;
#[cfg(feature = "debug-scenarios")]
mod intentional_panic;

use serde::Serialize;

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
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Hello => "hello",
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic => "panic",
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
    fn hook_install_is_idempotent() {
        install_panic_hook();
        install_panic_hook();
        let json = serde_json::to_value(execute("hello", Some(&debug_runtime()))).expect("json");
        assert_eq!(json["status"], "passed");
    }
}
