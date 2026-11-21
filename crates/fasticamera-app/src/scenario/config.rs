//! Настройки сценариев (секция `scenarios` конфига нативной части): значения по умолчанию, ошибки и проверка.
//! Вынесено из `mod.rs` ради размера модуля; пути `scenario::ScenarioConfig` и `scenario::ScenarioConfigError`
//! сохранены реэкспортом.

use serde::{Deserialize, Serialize};

use super::camera::{StandbyScenarioConfig, StandbyScenarioConfigError};
use crate::result_echo::{ResultEchoConfig, ResultEchoConfigError};

/// Настройки сценариев (секция `scenarios` конфига нативной части).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScenarioConfig {
    /// Сколько `motion` пишет гироскоп, мс. 3000 — как замер сессии 00 (1200 показаний).
    pub motion_duration_ms: u64,
    /// Пауза между паттернами хаптики в `platform`, мс: 700 — чтобы различить их на ощупь.
    pub haptic_interval_ms: u64,
    /// Сколько `platform` держит на экране тестовую подсказку владельцу, мс: 2000 — чтобы скрипт успел снять
    /// скриншот (стадия `operator_prompt_shown`) и убедиться, что текст дошёл до экрана.
    pub operator_prompt_hold_ms: u64,
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
    /// Сценарий `standby` (13b): сроки ожиданий и порог частоты тёплых камер.
    pub standby: StandbyScenarioConfig,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            motion_duration_ms: 3000,
            haptic_interval_ms: 700,
            operator_prompt_hold_ms: 2_000,
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
            standby: StandbyScenarioConfig::default(),
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
    #[error(transparent)]
    Standby(#[from] StandbyScenarioConfigError),
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
        if self.operator_prompt_hold_ms == 0 {
            return Err(ScenarioConfigError::Zero {
                field: "operator_prompt_hold_ms",
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
        self.standby.validate()?;
        Ok(())
    }
}
