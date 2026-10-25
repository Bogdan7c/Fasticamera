//! Состояние нативной части на процесс: принятый конфиг, установленные логи и хук паник.
//!
//! Конфиг фиксируется при первом `initialize` и дальше не меняется: Android может повторно
//! создать Activity в том же процессе, а глобальный подписчик tracing ставится один раз.
//! Повторный вызов с другим конфигом не молчит — он виден в отчёте (`requested_differs`).

use std::sync::OnceLock;

use serde::Serialize;

use crate::logging::{LoggingInstall, install_global_logging};
use crate::panic_report::install_panic_hook;
use crate::runtime_config::RuntimeConfig;

/// Нативная часть процесса. В продукте один экземпляр — `process_runtime()`;
/// отдельные экземпляры нужны тестам, чтобы проверять переходы состояния.
pub struct NativeRuntime {
    config: OnceLock<RuntimeConfig>,
}

/// Итог `initialize`. Поле `status` в JSON: `initialized` / `already_initialized` / `failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InitReport {
    Initialized {
        log_filter: String,
    },
    AlreadyInitialized {
        active_log_filter: String,
        requested_differs: bool,
    },
    Failed {
        error: String,
    },
}

static PROCESS_RUNTIME: NativeRuntime = NativeRuntime::new();

/// Нативная часть этого процесса.
pub fn process_runtime() -> &'static NativeRuntime {
    &PROCESS_RUNTIME
}

impl NativeRuntime {
    pub const fn new() -> Self {
        Self {
            config: OnceLock::new(),
        }
    }

    /// Принимает конфиг от Kotlin, ставит хук паник и логирование.
    /// Неверный конфиг — `Failed`, состояние не меняется (можно повторить с исправленным).
    pub fn initialize(&self, config_json: &str) -> InitReport {
        // Хук — первым: паника при разборе конфига тоже должна дойти до logcat.
        install_panic_hook();
        let requested = match RuntimeConfig::from_json(config_json) {
            Ok(config) => config,
            Err(error) => {
                return InitReport::Failed {
                    error: error.to_string(),
                };
            }
        };
        if let Some(active) = self.config.get() {
            return already_initialized(active, &requested);
        }
        if let Err(error) = install_global_logging(requested.log_filter()).map(log_reinstall) {
            return InitReport::Failed {
                error: error.to_string(),
            };
        }
        match self.config.set(requested.clone()) {
            Ok(()) => {
                tracing::info!(
                    target: "fasticamera::runtime",
                    build_type = ?requested.build_type(),
                    log_filter = requested.log_filter().directives(),
                    "нативная часть инициализирована"
                );
                InitReport::Initialized {
                    log_filter: requested.log_filter().directives().to_owned(),
                }
            }
            // Другой поток успел раньше: его конфиг действует, наш — сравниваем с ним.
            Err(_) => match self.config.get() {
                Some(active) => already_initialized(active, &requested),
                None => InitReport::Failed {
                    error: "конфиг не сохранён: OnceLock пуст после неудачного set".to_owned(),
                },
            },
        }
    }

    /// Принятый конфиг; `None` — `initialize` ещё не прошёл успешно.
    pub fn config(&self) -> Option<&RuntimeConfig> {
        self.config.get()
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn already_initialized(active: &RuntimeConfig, requested: &RuntimeConfig) -> InitReport {
    let requested_differs = active != requested;
    if requested_differs {
        tracing::warn!(
            target: "fasticamera::runtime",
            active = active.log_filter().directives(),
            requested = requested.log_filter().directives(),
            "повторный initialize с другим конфигом: действует первый (конфиг фиксирован на процесс)"
        );
    }
    InitReport::AlreadyInitialized {
        active_log_filter: active.log_filter().directives().to_owned(),
        requested_differs,
    }
}

/// Логирование уже стояло (другой экземпляр `NativeRuntime` в том же процессе — так бывает
/// только в тестах): фильтр остаётся прежним, об этом пишем в лог.
fn log_reinstall(install: LoggingInstall) {
    if let LoggingInstall::AlreadyInstalled { active_filter } = install {
        tracing::debug!(
            target: "fasticamera::runtime",
            active_filter = active_filter.directives(),
            "логирование уже установлено в процессе"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBUG_CONFIG: &str = r#"{"build_type":"debug","log_filter":"debug"}"#;

    #[test]
    fn first_initialize_stores_config() {
        let runtime = NativeRuntime::new();
        assert_eq!(
            runtime.initialize(DEBUG_CONFIG),
            InitReport::Initialized {
                log_filter: "debug".to_owned()
            }
        );
        assert_eq!(
            runtime.config().map(|config| config.log_filter().directives()),
            Some("debug")
        );
    }

    #[test]
    fn repeated_initialize_keeps_first_config_and_reports_difference() {
        let runtime = NativeRuntime::new();
        runtime.initialize(DEBUG_CONFIG);
        assert_eq!(
            runtime.initialize(DEBUG_CONFIG),
            InitReport::AlreadyInitialized {
                active_log_filter: "debug".to_owned(),
                requested_differs: false
            }
        );
        assert_eq!(
            runtime.initialize(r#"{"build_type":"debug","log_filter":"trace"}"#),
            InitReport::AlreadyInitialized {
                active_log_filter: "debug".to_owned(),
                requested_differs: true
            }
        );
        assert_eq!(
            runtime.config().map(|config| config.log_filter().directives()),
            Some("debug")
        );
    }

    #[test]
    fn invalid_config_fails_and_leaves_runtime_uninitialized() {
        let runtime = NativeRuntime::new();
        let report = runtime.initialize(r#"{"build_type":"debug","log_filter":"debgu"}"#);
        assert!(
            matches!(&report, InitReport::Failed { error } if error.contains("debgu")),
            "{report:?}"
        );
        assert!(runtime.config().is_none());
        // После ошибки можно повторить с исправленным конфигом.
        assert!(matches!(
            runtime.initialize(DEBUG_CONFIG),
            InitReport::Initialized { .. }
        ));
    }

    #[test]
    fn report_serializes_with_status_tag() {
        let json = serde_json::to_value(InitReport::Failed { error: "x".to_owned() }).expect("json");
        assert_eq!(json, serde_json::json!({"status": "failed", "error": "x"}));
    }
}
