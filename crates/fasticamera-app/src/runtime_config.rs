//! Конфиг нативной части, который Kotlin передаёт один раз при старте процесса.
//!
//! Источник значений — `android/gradle.properties` (дефолты по типу сборки) и intent-параметр
//! `log_filter` (переопределение на один запуск). В Rust ничего не хардкодится: здесь только
//! разбор и проверка.

use serde::{Deserialize, Serialize};
use tracing_subscriber::filter::Targets;

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
}

/// Проверенный конфиг нативной части.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeConfig {
    build_type: BuildType,
    log_filter: LogFilter,
}

impl RuntimeConfig {
    /// Разбирает JSON вида `{"build_type":"debug","log_filter":"debug"}`.
    pub fn from_json(config_json: &str) -> Result<Self, ConfigError> {
        let parsed: RuntimeConfigJson = serde_json::from_str(config_json)?;
        Ok(Self {
            build_type: parsed.build_type,
            log_filter: LogFilter::parse(&parsed.log_filter)?,
        })
    }

    pub fn build_type(&self) -> BuildType {
        self.build_type
    }

    pub fn log_filter(&self) -> &LogFilter {
        &self.log_filter
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
