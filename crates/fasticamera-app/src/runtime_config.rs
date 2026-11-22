//! Конфиг нативной части, который Kotlin передаёт один раз при старте процесса.
//!
//! Источник значений — `android/gradle.properties` (дефолты по типу сборки) и intent-параметр
//! `log_filter` (переопределение на один запуск). В Rust ничего не хардкодится: здесь только
//! разбор и проверка.
//!
//! Секции `shell`, `thermal_policy`, `scenarios`, `render`, `gpu` и `camera` необязательны: Kotlin их пока
//! не присылает, и действуют документированные значения по умолчанию владельцев (`ShellConfig`,
//! `ThermalPolicyConfig`, `ScenarioConfig`, `RenderLoopConfig`, `GpuConfig`, `CameraConfig`), а также `camera_screen` (экран камеры, `CameraScreenConfig`). Прислать можно частично — опечатка в имени поля
//! всё равно ошибка.

use android_shell::config::{ShellConfig, ShellConfigError, ValidatedShellConfig};
use render_loop::config::{ConfigError as RenderConfigError, RenderLoopConfig};
use serde::{Deserialize, Serialize};
use shell_contract::thermal_policy::{ThermalPolicy, ThermalPolicyConfig, ThermalPolicyConfigError};
use tracing_subscriber::filter::Targets;

use crate::camera_config::{CameraConfig, CameraConfigError};
use crate::camera_screen_config::{CameraScreenConfig, CameraScreenConfigError};
use crate::device_config::{DeviceConfig, DeviceIdentityError};
use crate::gpu_config::GpuConfig;
use crate::photo_config::{PhotoConfig, PhotoConfigError};
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
    #[serde(default)]
    render: RenderLoopConfig,
    #[serde(default)]
    gpu: GpuConfig,
    #[serde(default)]
    camera: CameraConfig,
    #[serde(default)]
    camera_screen: CameraScreenConfig,
    /// Паспорт телефона (`Build.MANUFACTURER`/`Build.MODEL`): Kotlin присылает его при каждом запуске.
    #[serde(default)]
    device: DeviceConfig,
    /// Служба записи снимков (сессия 14).
    #[serde(default)]
    photo: PhotoConfig,
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
    render: RenderLoopConfig,
    gpu: GpuConfig,
    camera: CameraConfig,
    camera_screen: CameraScreenConfig,
    device: DeviceConfig,
    photo: PhotoConfig,
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
        parsed.render.validate()?;
        parsed.camera.validate()?;
        parsed.camera_screen.validate()?;
        parsed.device.validate()?;
        // Предел очереди и сроки экрана со службой — конфиги двух crate-ов: согласуются здесь.
        parsed.photo.validate(&parsed.camera_screen.screen.capture)?;
        // Срок цели рендера против очереди кодеров службы: тоже конфиги двух crate-ов.
        parsed.photo.validate_against_render(&parsed.render.still)?;
        // Бюджет ожиданий GPU потока рендера обязан быть меньше ожидания квитанции оболочкой:
        // иначе `surfaceDestroyed` отпустит главный поток по таймауту раньше подтверждения.
        // Конфиги разных crate-ов, поэтому согласованность проверяется здесь, на стыке.
        if parsed.render.release_receipt_budget_ms >= parsed.shell.surface_release_timeout_ms {
            return Err(ConfigError::ReleaseBudgetNotBelowShellTimeout {
                render_budget_ms: parsed.render.release_receipt_budget_ms,
                shell_timeout_ms: parsed.shell.surface_release_timeout_ms,
            });
        }
        Ok(Self {
            build_type: parsed.build_type,
            log_filter: LogFilter::parse(&parsed.log_filter)?,
            shell: parsed.shell,
            validated_shell,
            thermal_policy: parsed.thermal_policy,
            scenarios: parsed.scenarios,
            render: parsed.render,
            gpu: parsed.gpu,
            camera: parsed.camera,
            camera_screen: parsed.camera_screen,
            device: parsed.device,
            photo: parsed.photo,
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

    /// Настройки потока рендера (пейсинг, таймауты, число кадров в полёте).
    pub fn render(&self) -> &RenderLoopConfig {
        &self.render
    }

    pub fn gpu(&self) -> &GpuConfig {
        &self.gpu
    }

    /// Настройки камеры: движок, бэкенд Camera2 и параметры сценариев.
    pub fn camera(&self) -> &CameraConfig {
        &self.camera
    }

    /// Экран камеры: включён ли и параметры его обвязки (сессия 12).
    pub fn camera_screen(&self) -> &CameraScreenConfig {
        &self.camera_screen
    }

    /// Паспорт телефона для тегов файлов снимка (может отсутствовать — это явное состояние).
    pub fn device(&self) -> &DeviceConfig {
        &self.device
    }

    /// Служба записи снимков: допуск, сроки, качество, каталог (сессия 14).
    pub fn photo(&self) -> &PhotoConfig {
        &self.photo
    }

    /// Режим validation слоя Vulkan: секция `gpu` с учётом типа сборки.
    pub fn validation_mode(&self) -> gpu_vulkan::validation::ValidationMode {
        self.gpu.validation_mode(self.build_type)
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
    #[error("неверные настройки потока рендера: {0}")]
    Render(#[from] RenderConfigError),
    #[error("неверные настройки камеры: {0}")]
    Camera(#[from] CameraConfigError),
    #[error("неверные настройки экрана камеры: {0}")]
    CameraScreen(#[from] CameraScreenConfigError),
    #[error("неверный паспорт телефона: {0}")]
    Device(#[from] DeviceIdentityError),
    #[error("неверные настройки службы снимков: {0}")]
    Photo(#[from] PhotoConfigError),
    #[error(
        "render.release_receipt_budget_ms ({render_budget_ms}) должен быть меньше shell.surface_release_timeout_ms ({shell_timeout_ms})"
    )]
    ReleaseBudgetNotBelowShellTimeout {
        render_budget_ms: u64,
        shell_timeout_ms: u64,
    },
}

#[cfg(test)]
#[path = "runtime_config_photo_tests.rs"]
mod photo_tests;

#[cfg(test)]
mod tests {
    use gpu_vulkan::validation::ValidationMode;

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
    fn render_and_gpu_sections_default_and_override() {
        let defaults = RuntimeConfig::from_json(r#"{"build_type":"release","log_filter":"info"}"#).expect("конфиг");
        assert_eq!(*defaults.render(), RenderLoopConfig::default());
        assert_eq!(*defaults.gpu(), GpuConfig::default());
        assert_eq!(defaults.validation_mode(), ValidationMode::Disabled, "release + auto");

        let debug_defaults = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
        assert_eq!(
            debug_defaults.validation_mode(),
            ValidationMode::full_with_known_deviations(GpuConfig::default().known_deviations()),
            "debug + auto: полная проверка с известными отступлениями по умолчанию"
        );

        let overridden = RuntimeConfig::from_json(
            r#"{"build_type":"release","log_filter":"info","render":{"pacing_window_s":4.0,"frames_in_flight":3},"gpu":{"validation":"enabled"}}"#,
        )
        .expect("конфиг с секциями render и gpu");
        assert_eq!(overridden.render().pacing_window_s, 4.0);
        assert_eq!(overridden.render().frames_in_flight, 3);
        assert_eq!(
            overridden.render().acquire_timeout_ms,
            RenderLoopConfig::default().acquire_timeout_ms,
            "непереопределённые поля берут значения по умолчанию"
        );
        assert!(overridden.validation_mode().wants_synchronization());
    }

    #[test]
    fn render_and_gpu_sections_reject_typos_and_bad_values() {
        let render_typo =
            RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","render":{"pacing_windo_s":1}}"#)
                .expect_err("опечатка в render");
        assert!(matches!(render_typo, ConfigError::Malformed(_)), "{render_typo}");
        let gpu_typo =
            RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","gpu":{"validaton":"auto"}}"#)
                .expect_err("опечатка в gpu");
        assert!(matches!(gpu_typo, ConfigError::Malformed(_)), "{gpu_typo}");
        let gpu_value =
            RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","gpu":{"validation":"maybe"}}"#)
                .expect_err("неизвестный режим");
        assert!(matches!(gpu_value, ConfigError::Malformed(_)), "{gpu_value}");
        let render_value =
            RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info","render":{"frames_in_flight":0}}"#)
                .expect_err("ноль кадров в полёте");
        assert!(matches!(render_value, ConfigError::Render(_)), "{render_value}");
    }

    /// Ручки тона видоискателя (сессия 12b) приходят из Kotlin как `render.viewfinder.tone` (intent-параметры
    /// `tone_contrast` / `tone_display_middle_gray`): значения доходят до конфига рендера, плохие — отказ запуска.
    #[test]
    fn viewfinder_tone_knobs_come_from_json_and_bad_values_fail_the_start() {
        let tuned = RuntimeConfig::from_json(
            r#"{"build_type":"release","log_filter":"info","render":{"viewfinder":{"tone":{"contrast":1.6,"display_middle_gray":0.22}}}}"#,
        )
        .expect("конфиг с ручками тона");
        assert_eq!(tuned.render().viewfinder.tone.contrast, 1.6);
        assert_eq!(tuned.render().viewfinder.tone.display_middle_gray, 0.22);

        let zero_contrast = RuntimeConfig::from_json(
            r#"{"build_type":"release","log_filter":"info","render":{"viewfinder":{"tone":{"contrast":0.0}}}}"#,
        )
        .expect_err("нулевой контраст");
        assert!(matches!(zero_contrast, ConfigError::Render(_)), "{zero_contrast}");
        assert!(zero_contrast.to_string().contains("viewfinder.tone"), "{zero_contrast}");

        // Kotlin кладёт нечисловое значение строкой — Rust отвергает его громко, а не берёт умолчание.
        let not_a_number = RuntimeConfig::from_json(
            r#"{"build_type":"release","log_filter":"info","render":{"viewfinder":{"tone":{"contrast":"1,5"}}}}"#,
        )
        .expect_err("контраст строкой");
        assert!(matches!(not_a_number, ConfigError::Malformed(_)), "{not_a_number}");
    }

    #[test]
    fn render_release_budget_must_stay_below_the_shell_receipt_timeout() {
        // По умолчанию 450 < 500: можно.
        RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("дефолты согласованы");
        // Бюджет рендера поднят выше ожидания оболочки: отвергается целиком.
        let render_too_slow = RuntimeConfig::from_json(
            r#"{"build_type":"debug","log_filter":"info","render":{"release_receipt_budget_ms":600,"teardown_timeout_ms":250}}"#,
        )
        .expect_err("бюджет выше ожидания оболочки");
        assert!(
            matches!(
                render_too_slow,
                ConfigError::ReleaseBudgetNotBelowShellTimeout {
                    render_budget_ms: 600,
                    shell_timeout_ms: 500
                }
            ),
            "{render_too_slow}"
        );
        // Оболочка, ждущая меньше бюджета рендера, — та же ошибка.
        let shell_too_impatient = RuntimeConfig::from_json(
            r#"{"build_type":"debug","log_filter":"info","shell":{"surface_release_timeout_ms":400}}"#,
        )
        .expect_err("оболочка нетерпеливее бюджета рендера");
        assert!(
            matches!(
                shell_too_impatient,
                ConfigError::ReleaseBudgetNotBelowShellTimeout { .. }
            ),
            "{shell_too_impatient}"
        );
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
