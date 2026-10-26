//! Конфиг нативной части, который Kotlin передаёт один раз при старте процесса.
//!
//! Источник значений — `android/gradle.properties` (дефолты по типу сборки) и intent-параметр
//! `log_filter` (переопределение на один запуск). В Rust ничего не хардкодится: здесь только
//! разбор и проверка.
//!
//! Секции `shell`, `thermal_policy` и `scenarios` необязательны: Kotlin их пока не присылает,
//! и действуют документированные значения по умолчанию владельцев (`ShellConfig`,
//! `ThermalPolicyConfig`, `ScenarioConfig`). Прислать можно частично — опечатка в имени поля
//! всё равно ошибка.

use android_shell::config::{ShellConfig, ShellConfigError, ValidatedShellConfig};
use serde::{Deserialize, Serialize};
use shell_contract::thermal_policy::{ThermalPolicy, ThermalPolicyConfig, ThermalPolicyConfigError};
use tracing_subscriber::filter::Targets;

use crate::scenario::ScenarioConfig;

/// Тип сборки APK (`BuildConfig.BUILD_TYPE` на стороне Kotlin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildType {
    Debug,
    Release,
}

/// Фильтр логов в синтаксисе `tracing_subscriber::filter::Targets`:
/// `"info"` или `"info,fasticamera::scenario=trace"`.
#[derive(Debug, Clone)]
pub struct LogFilter {
    /// Исходная строка — для отчётов и сравнения конфигов.
    directives: String,
    /// Разобранный фильтр, который получает подписчик tracing.
    targets: Targets,
}

impl LogFilter {
    /// Разбирает строку фильтра.
    ///
    /// Обязателен уровень по умолчанию (`info`, `debug`, ...). Без него `Targets` принял бы
    /// опечатку вроде `"debgu"` как имя цели, и логи молча пропали бы целиком.
    pub fn parse(directives: &str) -> Result<Self, ConfigError> {
        let trimmed = directives.trim();
        if trimmed.is_empty() {
            return Err(ConfigError::EmptyLogFilter);
        }
        let targets: Targets = trimmed.parse().map_err(|error| ConfigError::InvalidLogFilter {
            directives: trimmed.to_owned(),
            reason: format!("{error}"),
        })?;
        if targets.default_level().is_none() {
            return Err(ConfigError::InvalidLogFilter {
                directives: trimmed.to_owned(),
                reason: "нет уровня по умолчанию (например, `info`); без него неизвестные цели молчат".to_owned(),
            });
        }
        Ok(Self {
            directives: trimmed.to_owned(),
            targets,
        })
    }

    /// Строка фильтра в том виде, в каком её задали (без пробелов по краям).
    pub fn directives(&self) -> &str {
        &self.directives
    }

    /// Разобранный фильтр для подписчика tracing.
    pub fn targets(&self) -> &Targets {
        &self.targets
    }
}

impl PartialEq for LogFilter {
    /// Фильтры равны, если равны их строки: разбор детерминирован.
    fn eq(&self, other: &Self) -> bool {
        self.directives == other.directives
    }
}

/// Форма JSON, которую присылает Kotlin. Лишние поля — ошибка, а не молчаливый игнор.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfigJson {
    build_type: BuildType,
    log_filter: String,
    #[serde(default)]
    shell: ShellConfig,
    #[serde(default)]
    thermal_policy: ThermalPolicyConfig,
    #[serde(default)]
    scenarios: ScenarioConfig,
}

/// Проверенный конфиг нативной части.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeConfig {
    build_type: BuildType,
    log_filter: LogFilter,
    shell: ShellConfig,
    validated_shell: ValidatedShellConfig,
    thermal_policy: ThermalPolicyConfig,
    scenarios: ScenarioConfig,
}

impl RuntimeConfig {
    /// Разбирает JSON вида `{"build_type":"debug","log_filter":"debug"}`.
    pub fn from_json(config_json: &str) -> Result<Self, ConfigError> {
        let parsed: RuntimeConfigJson = serde_json::from_str(config_json)?;
        let validated_shell = parsed.shell.validate()?;
        // Пороги политики проверяет её конструктор; здесь — та же проверка заранее, чтобы
        // неверный конфиг отвергался целиком при initialize, а не позже при установке.
        ThermalPolicy::new(parsed.thermal_policy)?;
        parsed.scenarios.validate()?;
        Ok(Self {
            build_type: parsed.build_type,
            log_filter: LogFilter::parse(&parsed.log_filter)?,
            shell: parsed.shell,
            validated_shell,
            thermal_policy: parsed.thermal_policy,
            scenarios: parsed.scenarios,
        })
    }

    pub fn build_type(&self) -> BuildType {
        self.build_type
    }

    pub fn log_filter(&self) -> &LogFilter {
        &self.log_filter
    }

    /// Настройки оболочки в исходном виде (для `android_shell::android::install`).
    pub fn shell(&self) -> &ShellConfig {
        &self.shell
    }

    /// Те же настройки, проверенные (частоты, таймауты как `Duration`).
    pub fn validated_shell(&self) -> &ValidatedShellConfig {
        &self.validated_shell
    }

    pub fn thermal_policy(&self) -> ThermalPolicyConfig {
        self.thermal_policy
    }

    pub fn scenarios(&self) -> &ScenarioConfig {
        &self.scenarios
    }
}

/// Почему конфиг не принят.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("конфиг не разобран: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("пустой фильтр логов")]
    EmptyLogFilter,
    #[error("неверный фильтр логов {directives:?}: {reason}")]
    InvalidLogFilter { directives: String, reason: String },
    #[error("неверные настройки оболочки: {0}")]
    Shell(#[from] ShellConfigError),
    #[error("неверные пороги тепловой политики: {0}")]
    ThermalPolicy(#[from] ThermalPolicyConfigError),
    #[error("неверные настройки сценариев: {0}")]
    Scenarios(#[from] crate::scenario::ScenarioConfigError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_config() {
        let config = RuntimeConfig::from_json(r#"{"build_type":"release","log_filter":" info,fasticamera=debug "}"#)
            .expect("валидный конфиг");
        assert_eq!(config.build_type(), BuildType::Release);
        assert_eq!(config.log_filter().directives(), "info,fasticamera=debug");
        assert_eq!(
            config.log_filter().targets().default_level(),
            Some(tracing_subscriber::filter::LevelFilter::INFO)
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let error = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","extra":1}"#)
            .expect_err("лишнее поле");
        assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
        assert!(error.to_string().contains("extra"), "{error}");
    }

    #[test]
    fn optional_sections_take_defaults_and_accept_partial_override() {
        let config = RuntimeConfig::from_json(
            r#"{"build_type":"debug","log_filter":"info","shell":{"gyro_rate_hz":200},"scenarios":{"motion_duration_ms":1000}}"#,
        )
        .expect("конфиг с частичными секциями");
        assert_eq!(config.shell().gyro_rate_hz, 200);
        assert_eq!(config.validated_shell().default_gyro_request.rate.hertz(), 200);
        assert_eq!(
            config.shell().surface_release_timeout_ms,
            ShellConfig::default().surface_release_timeout_ms
        );
        assert_eq!(config.thermal_policy(), ThermalPolicyConfig::default());
        assert_eq!(config.scenarios().motion_duration_ms, 1000);
    }

    #[test]
    fn invalid_optional_sections_reject_whole_config() {
        let shell =
            RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","shell":{"gyro_rate_hz":0}}"#)
                .expect_err("нулевая частота");
        assert!(matches!(shell, ConfigError::Shell(_)), "{shell}");
        let policy = RuntimeConfig::from_json(
            r#"{"build_type":"debug","log_filter":"info","thermal_policy":{"elevated_from_headroom":2.0}}"#,
        )
        .expect_err("порог выше критического");
        assert!(matches!(policy, ConfigError::ThermalPolicy(_)), "{policy}");
        let typo = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","shell":{"gyro":1}}"#)
            .expect_err("опечатка в секции");
        assert!(matches!(typo, ConfigError::Malformed(_)), "{typo}");
    }

    #[test]
    fn rejects_unknown_build_type() {
        let error = RuntimeConfig::from_json(r#"{"build_type":"beta","log_filter":"info"}"#).expect_err("beta");
        assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
    }

    #[test]
    fn rejects_empty_filter() {
        assert!(matches!(LogFilter::parse("   "), Err(ConfigError::EmptyLogFilter)));
    }

    #[test]
    fn rejects_filter_without_default_level() {
        // Опечатка в уровне: Targets принял бы её как имя цели, и все логи молча пропали бы.
        let error = LogFilter::parse("debgu").expect_err("опечатка");
        assert!(matches!(error, ConfigError::InvalidLogFilter { .. }), "{error}");
        assert!(error.to_string().contains("debgu"), "{error}");
    }

    #[test]
    fn rejects_malformed_level_in_directive() {
        let error = LogFilter::parse("info,fasticamera=loud").expect_err("неверный уровень");
        assert!(matches!(error, ConfigError::InvalidLogFilter { .. }), "{error}");
    }
}
