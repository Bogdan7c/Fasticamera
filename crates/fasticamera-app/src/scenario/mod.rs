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

mod ahb_import;
mod background_job;
mod camera;
mod gpu_fence;
mod render;
mod render_lifecycle;
pub(crate) mod render_metrics;

use serde::{Deserialize, Serialize};

use crate::jni_boundary::run_guarded;
use crate::panic_report::PanicReport;
use crate::result_echo::{ResultEchoConfig, ResultEchoConfigError};
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
    /// Показ тестовой картинки: ритм 120 Гц, пейсинг, параметры swapchain.
    Render,
    /// Показ переживает сворачивания и блокировки экрана без утечек GPU-объектов.
    RenderLifecycle,
    /// Импорт RAW `AHardwareBuffer` в Vulkan без копий (RAW16 и RAW10).
    AhbImport,
    /// Порядок GPU-работ через `sync_fd`: B не стартует раньше конца A.
    GpuFence,
    Camera(camera::CameraScenario),
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
const DEBUG_ONLY_SCENARIO_NAMES: &[&str] = &["panic", "camera-evict", "raw-stream-cpu-probe"];

impl Scenario {
    /// Все сценарии этой сборки.
    pub fn available() -> Vec<Scenario> {
        let own = [
            Scenario::Hello,
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic,
            Scenario::Lifecycle,
            Scenario::Permission,
            Scenario::Motion,
            Scenario::Platform,
            Scenario::Render,
            Scenario::RenderLifecycle,
            Scenario::AhbImport,
            Scenario::GpuFence,
        ];
        let cameras = camera::CameraScenario::scenarios();
        own.into_iter().chain(cameras).collect()
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
            Scenario::Render => "render",
            Scenario::RenderLifecycle => "render-lifecycle",
            Scenario::AhbImport => "ahb-import",
            Scenario::GpuFence => "gpu-fence",
            Scenario::Camera(kind) => kind.name(),
        }
    }

    pub fn mode(self) -> ScenarioMode {
        match self {
            Scenario::Hello => ScenarioMode::Immediate,
            #[cfg(feature = "debug-scenarios")]
            Scenario::Panic => ScenarioMode::Immediate,
            Scenario::Lifecycle
            | Scenario::Permission
            | Scenario::Motion
            | Scenario::Platform
            | Scenario::Render
            | Scenario::RenderLifecycle
            | Scenario::AhbImport
            | Scenario::GpuFence
            | Scenario::Camera(_) => ScenarioMode::Observing,
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
            Scenario::Render => Some(Box::new(render::RenderScenario::new())),
            Scenario::RenderLifecycle => Some(Box::new(render_lifecycle::RenderLifecycleScenario::new())),
            Scenario::AhbImport => Some(Box::new(ahb_import::AhbImportScenario::new())),
            Scenario::GpuFence => Some(Box::new(gpu_fence::GpuFenceScenario::new())),
            Scenario::Camera(kind) => Some(kind.observing()),
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
    /// Какую частоту `platform` просит у surface, Гц: 60 — частота показа по умолчанию
    /// (решение владельца 36, как `render.preferred_frame_rate_hz`).
    pub preferred_frame_rate_hz: f32,
    /// Насколько реальный период обновления экрана может отличаться от периода желаемой частоты
    /// (`render.preferred_frame_rate_hz`), доля: 0,05 = ±5 %. Шире — экран не на той частоте
    /// (платформа не приняла просьбу о 120 Гц).
    pub render_refresh_tolerance_ratio: f64,
    /// Сколько `render` и `render-lifecycle` ждут нужного состояния рендера (показ, освобождение
    /// surface), мс. 15000: с запасом на анимации системы и разблокировку экрана.
    pub render_stage_timeout_ms: u64,
    /// Сколько `render` после начала показа ждёт, пока реальный период обновления экрана
    /// сойдётся с желаемой частотой (до начала окна пейсинга), мс. OxygenOS держит 60 Гц
    /// до первого касания и лишь потом поднимает 120 Гц; смок шлёт синтетическое касание
    /// на стадии `awaiting_refresh`. 10000: с запасом на переходное значение при подключении, колпак и
    /// реакцию системы на касание. Не сошлось — сценарий падает.
    pub render_refresh_settle_timeout_ms: u64,
    /// Сколько период экрана (и пейсинга) обязан непрерывно держаться в допуске, чтобы `render`
    /// считал частоту сошедшейся, мс. 1500: переходное значение при подключении живёт ~250 мс,
    /// колпак OxygenOS применяется примерно за то же время; полторы секунды их перекрывают.
    pub render_refresh_stable_ms: u64,
    /// Как часто `render`, пока частота не сошлась, пишет стадию `awaiting_refresh`, мс.
    /// 2000: достаточно, чтобы касание успело подействовать, и достаточно часто для повтора.
    pub render_refresh_retry_ms: u64,
    /// Сколько раз `render-lifecycle` сворачивает приложение кнопкой HOME и возвращает.
    pub render_lifecycle_cycles: u32,
    /// Сколько раз `render-lifecycle` гасит и включает экран. 0 — только HOME-циклы.
    pub render_lifecycle_screen_cycles: u32,
    /// Пауза после возобновления показа перед замером живых объектов, мс. 600 — больше двух
    /// периодов опроса диагностики рендера (250 мс): замер видит значения уже нового показа.
    pub render_settle_ms: u64,
    /// Размеры RAW-буфера `ahb-import`, пиксели: 4096×3072 — кадр основной камеры OnePlus 11 (сессия 00).
    pub ahb_width: u32,
    pub ahb_height: u32,
    /// Сколько `ahb-import` и `gpu-fence` ждут завершения GPU-работы, мс. 30000: заведомо больше
    /// любой штатной работы (импорт 4096×3072 и busy-цикл занимают миллисекунды), но конечно —
    /// зависшая работа даёт вердикт «не завершилась за N мс», а не вечное ожидание.
    pub gpu_job_timeout_ms: u64,
    /// Длина цикла работы A в `gpu-fence` (чем больше, тем дольше A и надёжнее проверка порядка).
    pub fence_busy_iterations: u32,
    /// Повтор строки результата наблюдающих сценариев (решение 58): logcat теряет строки во всплеске.
    pub result_echo: ResultEchoConfig,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            motion_duration_ms: 3000,
            haptic_interval_ms: 700,
            preferred_frame_rate_hz: 60.0,
            render_refresh_tolerance_ratio: 0.05,
            render_stage_timeout_ms: 15_000,
            render_refresh_settle_timeout_ms: 10_000,
            render_refresh_stable_ms: 1_500,
            render_refresh_retry_ms: 2_000,
            render_lifecycle_cycles: 20,
            render_lifecycle_screen_cycles: 3,
            render_settle_ms: 600,
            ahb_width: 4096,
            ahb_height: 3072,
            gpu_job_timeout_ms: 30_000,
            fence_busy_iterations: gpu_vulkan::sync::diagnostics::DEFAULT_BUSY_ITERATIONS,
            result_echo: ResultEchoConfig::default(),
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
    #[error("render_refresh_tolerance_ratio должно лежать в (0, 1), а не {0}")]
    InvalidTolerance(f64),
    /// Устойчивость должна быть короче окна ожидания: иначе частота не успеет «сойтись» никогда.
    #[error(
        "render_refresh_stable_ms ({stable_ms}) должно быть меньше render_refresh_settle_timeout_ms ({settle_timeout_ms})"
    )]
    StableNotBelowSettleTimeout { stable_ms: u64, settle_timeout_ms: u64 },
    #[error(transparent)]
    ResultEcho(#[from] ResultEchoConfigError),
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
        if !(self.render_refresh_tolerance_ratio > 0.0 && self.render_refresh_tolerance_ratio < 1.0) {
            return Err(ScenarioConfigError::InvalidTolerance(
                self.render_refresh_tolerance_ratio,
            ));
        }
        for (field, value) in [
            ("render_stage_timeout_ms", self.render_stage_timeout_ms),
            (
                "render_refresh_settle_timeout_ms",
                self.render_refresh_settle_timeout_ms,
            ),
            ("render_refresh_stable_ms", self.render_refresh_stable_ms),
            ("render_refresh_retry_ms", self.render_refresh_retry_ms),
            ("render_settle_ms", self.render_settle_ms),
            ("gpu_job_timeout_ms", self.gpu_job_timeout_ms),
            ("ahb_width", u64::from(self.ahb_width)),
            ("ahb_height", u64::from(self.ahb_height)),
            ("fence_busy_iterations", u64::from(self.fence_busy_iterations)),
        ] {
            if value == 0 {
                return Err(ScenarioConfigError::Zero { field });
            }
        }
        if self.render_refresh_stable_ms >= self.render_refresh_settle_timeout_ms {
            return Err(ScenarioConfigError::StableNotBelowSettleTimeout {
                stable_ms: self.render_refresh_stable_ms,
                settle_timeout_ms: self.render_refresh_settle_timeout_ms,
            });
        }
        if self.render_lifecycle_cycles == 0 {
            return Err(ScenarioConfigError::Zero {
                field: "render_lifecycle_cycles",
            });
        }
        self.result_echo.validate()?;
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
    Passed {
        report: serde_json::Value,
    },
    Failed {
        error: String,
    },
    /// Сценарий провален, но собрал отчёт (его нужно видеть, чтобы понять, почему провал).
    /// В JSON выглядит как обычный `failed` с дополнительным полем `report`.
    #[serde(rename = "failed")]
    FailedWithReport {
        error: String,
        report: serde_json::Value,
    },
    Panicked {
        panic: PanicReport,
    },
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
        Scenario::Lifecycle
        | Scenario::Permission
        | Scenario::Motion
        | Scenario::Platform
        | Scenario::Render
        | Scenario::RenderLifecycle
        | Scenario::AhbImport
        | Scenario::GpuFence
        | Scenario::Camera(_) => Err(ScenarioError::NotImmediate { name: scenario.name() }),
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
    fn gpu_scenarios_are_registered_observing_and_product_diagnostics() {
        for name in ["render", "render-lifecycle", "ahb-import", "gpu-fence"] {
            let scenario = Scenario::parse(name).expect("сценарий зарегистрирован");
            assert_eq!(scenario.name(), name);
            assert_eq!(scenario.mode(), ScenarioMode::Observing);
            let observing = scenario.observing().expect("наблюдающий сценарий");
            assert_eq!(observing.name(), name, "имя сценария и его реализации совпадают");
        }
        // Не только debug-scenarios: доступны и в сборке без этой фичи.
        assert!(!DEBUG_ONLY_SCENARIO_NAMES.contains(&"render"));
        let json = serde_json::to_value(execute("gpu-fence", Some(&debug_runtime()))).expect("json");
        assert!(json["error"].as_str().expect("error").contains("маршрутизатор"));
    }

    #[test]
    fn gpu_scenario_config_defaults_and_validation() {
        let defaults = ScenarioConfig::default();
        assert_eq!(defaults.render_lifecycle_cycles, 20);
        assert_eq!(defaults.render_lifecycle_screen_cycles, 3);
        assert_eq!((defaults.ahb_width, defaults.ahb_height), (4096, 3072));
        assert_eq!(defaults.render_refresh_settle_timeout_ms, 10_000);
        assert_eq!(defaults.render_refresh_stable_ms, 1_500);
        assert_eq!(defaults.render_refresh_retry_ms, 2_000);
        assert!(defaults.validate().is_ok());
        for broken in [
            ScenarioConfig {
                render_refresh_tolerance_ratio: 0.0,
                ..defaults
            },
            ScenarioConfig {
                render_refresh_tolerance_ratio: 1.5,
                ..defaults
            },
            ScenarioConfig {
                render_refresh_tolerance_ratio: f64::NAN,
                ..defaults
            },
        ] {
            assert!(
                matches!(broken.validate(), Err(ScenarioConfigError::InvalidTolerance(_))),
                "{broken:?}"
            );
        }
        for (broken, field) in [
            (
                ScenarioConfig {
                    render_lifecycle_cycles: 0,
                    ..defaults
                },
                "render_lifecycle_cycles",
            ),
            (
                ScenarioConfig {
                    render_stage_timeout_ms: 0,
                    ..defaults
                },
                "render_stage_timeout_ms",
            ),
            (
                ScenarioConfig {
                    render_refresh_settle_timeout_ms: 0,
                    ..defaults
                },
                "render_refresh_settle_timeout_ms",
            ),
            (
                ScenarioConfig {
                    render_refresh_stable_ms: 0,
                    ..defaults
                },
                "render_refresh_stable_ms",
            ),
            (
                ScenarioConfig {
                    render_refresh_retry_ms: 0,
                    ..defaults
                },
                "render_refresh_retry_ms",
            ),
            (
                ScenarioConfig {
                    ahb_width: 0,
                    ..defaults
                },
                "ahb_width",
            ),
            (
                ScenarioConfig {
                    fence_busy_iterations: 0,
                    ..defaults
                },
                "fence_busy_iterations",
            ),
        ] {
            assert_eq!(broken.validate(), Err(ScenarioConfigError::Zero { field }));
        }
        // Ноль экранных циклов разрешён: только HOME.
        assert!(
            ScenarioConfig {
                render_lifecycle_screen_cycles: 0,
                ..defaults
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn refresh_stable_time_must_be_shorter_than_settle_timeout() {
        let defaults = ScenarioConfig::default();
        // Равенство тоже отвергается: за всё окно ожидания устойчивость набрать не успеть.
        for stable_ms in [defaults.render_refresh_settle_timeout_ms, 20_000] {
            let broken = ScenarioConfig {
                render_refresh_stable_ms: stable_ms,
                ..defaults
            };
            assert_eq!(
                broken.validate(),
                Err(ScenarioConfigError::StableNotBelowSettleTimeout {
                    stable_ms,
                    settle_timeout_ms: defaults.render_refresh_settle_timeout_ms,
                })
            );
        }
        let just_below = ScenarioConfig {
            render_refresh_stable_ms: defaults.render_refresh_settle_timeout_ms - 1,
            ..defaults
        };
        assert!(just_below.validate().is_ok());
    }

    #[test]
    fn gpu_job_timeout_has_default_and_rejects_zero() {
        assert_eq!(ScenarioConfig::default().gpu_job_timeout_ms, 30_000);
        let zero = ScenarioConfig {
            gpu_job_timeout_ms: 0,
            ..ScenarioConfig::default()
        };
        assert_eq!(
            zero.validate(),
            Err(ScenarioConfigError::Zero {
                field: "gpu_job_timeout_ms"
            })
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
