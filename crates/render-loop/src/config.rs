//! Конфигурация потока рендера.
//!
//! Все числа, от которых зависит поведение цикла, живут здесь, а не в логике (правило
//! репозитория «не хардкодить конфиг»). Структура читается из JSON/TOML через serde:
//! неизвестные поля — ошибка (опечатка в ключе не должна молча дать значение по умолчанию),
//! отсутствующие поля берутся из [`Default`].

use std::time::Duration;

use serde::{Deserialize, Serialize};

mod adpf;
mod auto_exposure;
mod continuity;
mod lens_shading;
mod look;
pub(crate) mod look_lut;
mod present_target;
mod scopes;
mod still;
mod tone;
mod ui_overlay;
mod unchanged_frame;
mod viewfinder;
mod white_balance;

pub use adpf::{AdpfConfig, AdpfConfigError, AdpfMode, MAX_ADPF_GPU_WAIT_FRAMES, MIN_ADPF_TARGET_FRACTION};

pub use auto_exposure::{AutoExposureConfig, AutoExposureConfigError};
pub use continuity::{ContinuityConfig, ContinuityConfigError, RevealExposureCheck};
pub use lens_shading::{LensShadingMode, ViewfinderLensShadingConfig};
pub use look::LookConfigError;
pub use look_lut::{LookLutLimitConflict, look_lut_limits};
pub use scopes::{MAX_SCOPES_EVERY_NTH_FRAME, ScopesConfig, ScopesConfigError};
pub use still::{StillConfigError, StillDevelopConfig};
pub use tone::{ToneBaseChoice, ToneConfigError, ViewfinderToneConfig};
pub use ui_overlay::{UiOverlayConfig, UiOverlayConfigError};
pub use unchanged_frame::{
    MAX_CONSECUTIVE_SKIPS_LIMIT, UnchangedFrameConfig, UnchangedFrameConfigError, UnchangedFramePolicy,
};
pub use viewfinder::{
    CropAspectConfig, DisplayColorConfig, MAX_VIEWFINDER_GPU_TIME_WINDOW_FRAMES, ViewfinderConfig,
    ViewfinderConfigError, ViewfinderCropConfig, ViewfinderPlacement,
};
pub use white_balance::{WhiteBalanceConfig, WhiteBalanceConfigError};

pub use present_target::{
    DEFAULT_NEAREST_FEASIBLE_MARGIN_US, DepthReductionMode, LateLatchMode, MAX_DEPTH_REDUCTION_STREAK_FRAMES,
    MAX_PRESENT_TARGET_DURATION_US, MAX_PRESENT_TARGET_WINDOW_FRAMES, PresentTargetConfig, PresentTargetConfigError,
    PresentTimelinePolicy,
};

/// Что не так с конфигурацией (находится в [`RenderLoopConfig::validate`]).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// Частота кадров — конечное число в [`MIN_FRAME_RATE_HZ`, `MAX_FRAME_RATE_HZ`]. Крайние
    /// значения вроде 1e-30 дают период, не помещающийся в `Duration` (паника при построении).
    #[error("preferred_frame_rate_hz должен лежать в [{MIN_FRAME_RATE_HZ}, {MAX_FRAME_RATE_HZ}] Гц, а не {0}")]
    FrameRateOutOfRange(f32),
    /// Число кадров в полёте — хотя бы один, иначе рисовать нечем.
    #[error("frames_in_flight должен быть не меньше 1")]
    ZeroFramesInFlight,
    /// Таймаут, равный нулю, превращает ожидание в busy-loop.
    #[error("{field} должен быть больше нуля миллисекунд")]
    ZeroTimeout { field: &'static str },
    /// Размер буфера без места не хранит ничего: наблюдатель остался бы без данных.
    #[error("{field} должен быть больше нуля")]
    ZeroCapacity { field: &'static str },
    /// Допустимая доля пропусков — вероятность, то есть число от 0 до 1.
    #[error("pacing_max_miss_ratio должен лежать в [0, 1], а не {0}")]
    MissRatioOutOfRange(f64),
    /// Окно статистики — число секунд в (0, [`MAX_PACING_WINDOW_S`]]. Огромные значения
    /// не помещаются в `Duration` (паника при построении).
    #[error("pacing_window_s должен лежать в (0, {MAX_PACING_WINDOW_S}] секунд, а не {0}")]
    WindowOutOfRange(f64),
    /// Минимальная полнота данных — доля в (0, 1]: ноль отключил бы проверку.
    #[error("pacing_min_coverage_ratio должен лежать в (0, 1], а не {0}")]
    CoverageRatioOutOfRange(f64),
    /// Худший путь до квитанции (самое долгое уже начатое ожидание GPU + `detach`) не должен
    /// превышать бюджет квитанции освобождения.
    #[error(
        "max(acquire_timeout_ms ({acquire_timeout_ms}), teardown_timeout_ms ({teardown_timeout_ms})) + teardown_timeout_ms превышают release_receipt_budget_ms ({budget_ms})"
    )]
    ReleaseBudgetExceeded {
        acquire_timeout_ms: u64,
        teardown_timeout_ms: u64,
        budget_ms: u64,
    },
    /// Порог смены периода — доля от периода, строго между 0 и 0,5.
    #[error("refresh_period_change_ratio должен лежать в (0, 0.5), а не {0}")]
    PeriodChangeRatioOutOfRange(f64),
    /// Упреждение метки показа — доля периода в [0, 1): целый период и больше сдвинул бы
    /// кадр на vsync раньше задуманного.
    #[error("present_time_lead_ratio должен лежать в [0, 1), а не {0}")]
    PresentLeadRatioOutOfRange(f64),
    /// Ошибка в секции выбора линии кадра и позднего захвата.
    #[error(transparent)]
    PresentTarget(#[from] PresentTargetConfigError),
    /// Ошибка в секции видоискателя.
    #[error(transparent)]
    Viewfinder(#[from] ViewfinderConfigError),
    /// Ошибка в секции подсказок производительности (ADPF).
    #[error(transparent)]
    Adpf(#[from] AdpfConfigError),
    /// Ошибка в секции UI-прохода поверх видоискателя.
    #[error(transparent)]
    UiOverlay(#[from] UiOverlayConfigError),
    /// Ошибка в секции пропуска тиков без изменений.
    #[error(transparent)]
    UnchangedFrame(#[from] UnchangedFrameConfigError),
    /// Ошибка в секции снимка.
    #[error(transparent)]
    Still(#[from] StillConfigError),
    /// Предел таблицы вида задан не в модели вида и расходится с ней.
    #[error(transparent)]
    LookLutLimit(#[from] LookLutLimitConflict),
}

/// Наименьшая допустимая `preferred_frame_rate_hz`, Гц. Реальные режимы экрана — 24..=240.
pub const MIN_FRAME_RATE_HZ: f32 = 1.0;
/// Наибольшая допустимая `preferred_frame_rate_hz`, Гц.
pub const MAX_FRAME_RATE_HZ: f32 = 1000.0;
/// Наибольшее окно статистики пейсинга, секунд (час: дальше окно теряет смысл как «скользящее»).
pub const MAX_PACING_WINDOW_S: f64 = 3600.0;

/// Параметры потока рендера. Значения по умолчанию описаны у каждого поля.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RenderLoopConfig {
    /// Частота обновления экрана, которую просим у платформы, Гц. 60 по умолчанию (решение
    /// владельца 36): поток камеры идёт на 60 кадр/с, бюджет GPU на кадр 16,7 мс (V4), меньше
    /// нагрев (V8); потолок экрана OnePlus 11 (120) остаётся опцией конфига. Кроме того, при
    /// 60 не важен «колпак» OxygenOS в 60 Гц до первого касания. По этой частоте же считается
    /// запасной период для статистики пейсинга.
    pub preferred_frame_rate_hz: f32,
    /// Сколько кадров может одновременно быть «в полёте» (на CPU и GPU). 2 — двойная
    /// буферизация: минимум задержки без простоя CPU. Читает бэкенд при `attach`.
    pub frames_in_flight: u32,
    /// Сколько ждать картинку swapchain (acquire), мс. 100 — заведомо больше периода
    /// кадра, но меньше таймаута квитанции surface (500 мс), чтобы не задерживать release.
    pub acquire_timeout_ms: u64,
    /// Сколько ждать тик vsync, мс. 50 — несколько пропущенных периодов при 60 Гц;
    /// по истечении поток проверяет почтовый ящик, а не зависает.
    pub vsync_wait_timeout_ms: u64,
    /// Допустимая доля пропущенных кадров в окне (интервал > 1,5 периода). 0,005 = 0,5 %.
    pub pacing_max_miss_ratio: f64,
    /// Длина скользящего окна статистики пейсинга, секунд; допустимо (0, 3600].
    pub pacing_window_s: f64,
    /// Минимальная полнота данных пейсинга: `intervals / frames_covered` (какая доля кадров окна
    /// дала честный интервал; остальное — дыры в записях драйвера и разрывы цепочки). Ниже
    /// порога вердикт не «Passed», а «недостаточно данных»: иначе разреженная статистика
    /// (много потерянных записей, мало интервалов) могла бы пройти по доле пропусков из горстки
    /// интервалов. 0,9 — на телефоне наблюдается 601 из 602 (≈ 0,998). Допустимо (0, 1].
    pub pacing_min_coverage_ratio: f64,
    /// Как часто опрашивать диагностику бэкенда (живые объекты, validation, времена показа), мс.
    /// Опрос не на каждом кадре: он может быть дорогим (вызовы драйвера).
    pub diagnostics_poll_interval_ms: u64,
    /// Бюджет ожидания GPU при пересоздании и отсоединении цепочки показа, мс. Истёк — Vulkan-
    /// объекты НЕ уничтожаются (уничтожать занятое нельзя), `detach` сообщает ошибку, а окно
    /// всё равно возвращается. Обязан быть ЗАМЕТНО меньше бюджета квитанции оболочки
    /// (`surface_release_timeout_ms` в android-shell, по умолчанию 500): иначе оболочка
    /// отпустит главный поток по таймауту раньше, чем поток рендера успеет подтвердить.
    /// 150 — с запасом: на телефоне вся разборка до квитанции занимает до 56 мс (сессия 03),
    /// и даже два ожидания подряд (уже идущее пересоздание + `detach`) укладываются в бюджет.
    pub teardown_timeout_ms: u64,
    /// Бюджет, в который обязаны уложиться ожидания GPU до подтверждения квитанции
    /// освобождения: `max(acquire_timeout_ms, teardown_timeout_ms) + teardown_timeout_ms <=
    /// release_receipt_budget_ms` (проверяет `validate`). Худший путь после `surfaceDestroyed`:
    /// ожидание GPU, уже начатое до прихода события (acquire или пересоздание цепочки), плюс
    /// `detach`; новые ожидания поток не начинает, если в ящике лежит уничтожение surface. ОБЯЗАН оставаться меньше `shell.surface_release_timeout_ms`
    /// (по умолчанию 500): оболочка не должна отпускать главный поток по таймауту раньше
    /// подтверждения. 450 — запас 50 мс на остальной путь.
    pub release_receipt_budget_ms: u64,
    /// Минимальный интервал между повторными пересоздания цепочки, которые поток запрашивает сам
    /// (после `NeedsRecreate`), мс. Нужен, когда пересоздание откладывается (нулевой размер окна):
    /// без него поток дёргал бы драйвер на каждом тике vsync. Смена размера от оболочки
    /// (`surfaceChanged`) интервал сбрасывает и применяется сразу.
    pub recreate_retry_interval_ms: u64,
    /// Насколько (в долях периода) должен отличаться новый период экрана от установленного,
    /// чтобы считаться СМЕНОЙ режима дисплея, а не дрожанием измерения. Драйвер Adreno
    /// отдаёт период с шумом в единицы микросекунд (16666, 16675, 16674 мкс…); без порога
    /// каждый опрос очищал бы окно пейсинга, и оно не копилось бы никогда. 0,02 = 2 %:
    /// заведомо больше шума (~0,05 %) и заведомо меньше разницы режимов (60 vs 90 Гц = 50 %).
    /// Допустимо открытое множество (0, 0,5).
    pub refresh_period_change_ratio: f64,
    /// Держать ли экран включённым, пока идёт показ (видоискатель камеры не должен гаснуть
    /// по системному таймауту). Поток рендера шлёт команду только при переходах «начал /
    /// закончил показ», не на каждом кадре.
    pub keep_screen_on_while_presenting: bool,
    /// На какую долю периода обновления желаемое время показа (`desiredPresentTime`) ставится
    /// раньше ожидаемого vsync от Choreographer. 0,5 — середина интервала перед целевым vsync:
    /// дрожание метки меньше полупериода не переносит кадр на следующий vsync. Без упреждения
    /// на OnePlus 11 было 4–5 % кадров по 2 периода и столько же выброшенных (сессия 03,
    /// `vulkan_backend::present_time`). Допустимо [0, 1).
    pub present_time_lead_ratio: f64,
    /// Сколько последних реальных времён показа (`present_id` -> момент на экране) поток рендера
    /// держит в статусе для наблюдателей. По ним приложение считает задержку «сенсор -> дисплей»
    /// (сопоставляет `present_id` с меткой сенсора из диагностики камеры). Наблюдатель опрашивает
    /// статус раз в десятки мс, а показов 60-120 в секунду: 256 записей (2-4 с) переживают любой
    /// разумный интервал опроса. Должно быть больше нуля.
    pub recent_presentations_capacity: usize,
    /// Выбор линии кадра Choreographer и момент позднего захвата кадра камеры (сессия 07).
    /// По умолчанию — предпочитаемая платформой линия, как раньше.
    pub present_target: PresentTargetConfig,
    /// Какое цветовое пространство просить у цепочки показа (сессия 07). По умолчанию Display P3,
    /// если экран его предлагает: ISP видоискателя кодирует выход под выбранное пространство.
    pub display_color: DisplayColorConfig,
    /// Видоискатель камеры: рисовальщик, ISP, кадрирование, положение, окна времени GPU (сессия 07).
    pub viewfinder: ViewfinderConfig,
    /// Подсказки производительности системе (ADPF, P13 сессии 07). По умолчанию выключены.
    pub adpf: AdpfConfig,
    /// UI и стекло поверх видоискателя (сессия 11): пределы кадра и пирамида размытия. Сам проход
    /// выключен, пока не установлен источник кадра UI (`RenderInbox::ui_overlay_command`).
    pub ui_overlay: UiOverlayConfig,
    /// Тики без изменений картинки не рисуются и не показываются (решение 214, сессия 16a). JSON без ключа —
    /// умолчания секции (пропуск выключен до замера на телефоне, решение 214).
    pub unchanged_frame: UnchangedFrameConfig,
    /// Снимок без задержки затвора (сессия 14): выбор кадра, очередь, проявление по полосам. JSON без ключа —
    /// умолчания секции.
    pub still: StillDevelopConfig,
}

impl Default for RenderLoopConfig {
    fn default() -> Self {
        Self {
            preferred_frame_rate_hz: 60.0,
            frames_in_flight: 2,
            acquire_timeout_ms: 100,
            vsync_wait_timeout_ms: 50,
            pacing_max_miss_ratio: 0.005,
            pacing_window_s: 10.0,
            pacing_min_coverage_ratio: 0.9,
            diagnostics_poll_interval_ms: 250,
            teardown_timeout_ms: 150,
            release_receipt_budget_ms: 450,
            recreate_retry_interval_ms: 100,
            refresh_period_change_ratio: 0.02,
            keep_screen_on_while_presenting: true,
            present_time_lead_ratio: 0.5,
            recent_presentations_capacity: 256,
            present_target: PresentTargetConfig::default(),
            display_color: DisplayColorConfig::default(),
            viewfinder: ViewfinderConfig::default(),
            adpf: AdpfConfig::default(),
            ui_overlay: UiOverlayConfig::default(),
            unchanged_frame: UnchangedFrameConfig::default(),
            still: StillDevelopConfig::default(),
        }
    }
}

impl RenderLoopConfig {
    /// Проверяет согласованность значений; поток рендера не стартует с неверным конфигом.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(MIN_FRAME_RATE_HZ..=MAX_FRAME_RATE_HZ).contains(&self.preferred_frame_rate_hz) {
            return Err(ConfigError::FrameRateOutOfRange(self.preferred_frame_rate_hz));
        }
        if self.frames_in_flight == 0 {
            return Err(ConfigError::ZeroFramesInFlight);
        }
        for (field, value) in [
            ("acquire_timeout_ms", self.acquire_timeout_ms),
            ("vsync_wait_timeout_ms", self.vsync_wait_timeout_ms),
            ("diagnostics_poll_interval_ms", self.diagnostics_poll_interval_ms),
            ("teardown_timeout_ms", self.teardown_timeout_ms),
            ("release_receipt_budget_ms", self.release_receipt_budget_ms),
            ("recreate_retry_interval_ms", self.recreate_retry_interval_ms),
        ] {
            if value == 0 {
                return Err(ConfigError::ZeroTimeout { field });
            }
        }
        // Сравнение через contains отвергает и NaN: он не лежит ни в каком диапазоне.
        if !(0.0..=1.0).contains(&self.pacing_max_miss_ratio) {
            return Err(ConfigError::MissRatioOutOfRange(self.pacing_max_miss_ratio));
        }
        if !(self.pacing_window_s > 0.0 && self.pacing_window_s <= MAX_PACING_WINDOW_S) {
            return Err(ConfigError::WindowOutOfRange(self.pacing_window_s));
        }
        if !(self.pacing_min_coverage_ratio > 0.0 && self.pacing_min_coverage_ratio <= 1.0) {
            return Err(ConfigError::CoverageRatioOutOfRange(self.pacing_min_coverage_ratio));
        }
        if self.worst_case_wait_before_release_ms() > self.release_receipt_budget_ms {
            return Err(ConfigError::ReleaseBudgetExceeded {
                acquire_timeout_ms: self.acquire_timeout_ms,
                teardown_timeout_ms: self.teardown_timeout_ms,
                budget_ms: self.release_receipt_budget_ms,
            });
        }
        // Через contains: NaN и границы отвергаются (интервал открытый).
        if !(self.refresh_period_change_ratio > 0.0 && self.refresh_period_change_ratio < 0.5) {
            return Err(ConfigError::PeriodChangeRatioOutOfRange(
                self.refresh_period_change_ratio,
            ));
        }
        if !(0.0..1.0).contains(&self.present_time_lead_ratio) {
            return Err(ConfigError::PresentLeadRatioOutOfRange(self.present_time_lead_ratio));
        }
        if self.recent_presentations_capacity == 0 {
            return Err(ConfigError::ZeroCapacity {
                field: "recent_presentations_capacity",
            });
        }
        self.present_target.validate()?;
        self.viewfinder.validate()?;
        self.adpf.validate()?;
        self.ui_overlay.validate()?;
        self.unchanged_frame.validate()?;
        self.still.validate()?;
        // Предел таблицы вида один на видоискатель, стенд и снимок (модель вида), а не три независимых числа.
        look_lut::validate_single_source(&self.viewfinder, &self.still)?;
        Ok(())
    }

    /// Худшее время ожиданий GPU от прихода `surfaceDestroyed` до квитанции, мс: уже начатое
    /// ожидание (acquire или пересоздание цепочки — что дольше) плюс ожидание `detach`.
    pub fn worst_case_wait_before_release_ms(&self) -> u64 {
        self.acquire_timeout_ms
            .max(self.teardown_timeout_ms)
            .saturating_add(self.teardown_timeout_ms)
    }

    /// Таймаут acquire картинки swapchain.
    pub fn acquire_timeout(&self) -> Duration {
        Duration::from_millis(self.acquire_timeout_ms)
    }

    /// Таймаут ожидания тика vsync.
    pub fn vsync_wait_timeout(&self) -> Duration {
        Duration::from_millis(self.vsync_wait_timeout_ms)
    }

    /// Период опроса диагностики бэкенда.
    pub fn diagnostics_poll_interval(&self) -> Duration {
        Duration::from_millis(self.diagnostics_poll_interval_ms)
    }

    /// Бюджет ожидания GPU при пересоздании и отсоединении цепочки.
    pub fn teardown_timeout(&self) -> Duration {
        Duration::from_millis(self.teardown_timeout_ms)
    }

    /// Минимальный интервал между самостоятельными пересозданиями цепочки.
    pub fn recreate_retry_interval(&self) -> Duration {
        Duration::from_millis(self.recreate_retry_interval_ms)
    }

    /// Длина окна статистики пейсинга.
    pub fn pacing_window(&self) -> Duration {
        Duration::from_secs_f64(self.pacing_window_s)
    }

    /// Ожидаемый период обновления экрана при `preferred_frame_rate_hz`.
    pub fn expected_refresh_period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.preferred_frame_rate_hz))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_brief() {
        let config = RenderLoopConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.preferred_frame_rate_hz, 60.0);
        assert_eq!(config.frames_in_flight, 2);
        assert_eq!(config.acquire_timeout(), Duration::from_millis(100));
        assert_eq!(config.vsync_wait_timeout(), Duration::from_millis(50));
        assert_eq!(config.pacing_max_miss_ratio, 0.005);
        assert_eq!(config.pacing_window(), Duration::from_secs(10));
    }

    #[test]
    fn default_teardown_budget_is_safely_below_the_shell_release_receipt_budget() {
        /// Бюджет квитанции в android-shell по умолчанию (`surface_release_timeout_ms`); конфиг
        /// оболочки живёт в другом crate, поэтому значение зеркалится здесь для проверки.
        const SHELL_RELEASE_RECEIPT_BUDGET: Duration = Duration::from_millis(500);
        let teardown = RenderLoopConfig::default().teardown_timeout();
        assert!(
            teardown + Duration::from_millis(100) <= SHELL_RELEASE_RECEIPT_BUDGET,
            "teardown {teardown:?} не оставляет запаса до таймаута квитанции оболочки"
        );
    }

    #[test]
    fn zero_teardown_or_retry_interval_is_rejected() {
        for (field, change) in [
            (
                "teardown_timeout_ms",
                (|c: &mut RenderLoopConfig| c.teardown_timeout_ms = 0) as fn(&mut RenderLoopConfig),
            ),
            ("recreate_retry_interval_ms", |c| c.recreate_retry_interval_ms = 0),
        ] {
            let mut config = RenderLoopConfig::default();
            change(&mut config);
            assert_eq!(config.validate(), Err(ConfigError::ZeroTimeout { field }));
        }
    }

    #[test]
    fn period_change_ratio_must_be_inside_open_interval() {
        for bad in [0.0, 0.5, -0.1, 0.7, f64::NAN, f64::INFINITY] {
            let config = RenderLoopConfig {
                refresh_period_change_ratio: bad,
                ..RenderLoopConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(ConfigError::PeriodChangeRatioOutOfRange(_))),
                "{bad} должен отвергаться"
            );
        }
        let config = RenderLoopConfig {
            refresh_period_change_ratio: 0.1,
            ..RenderLoopConfig::default()
        };
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(RenderLoopConfig::default().refresh_period_change_ratio, 0.02);
        assert!(RenderLoopConfig::default().keep_screen_on_while_presenting);
    }

    #[test]
    fn present_time_lead_ratio_must_be_inside_half_open_unit_interval() {
        for bad in [-0.1, 1.0, 1.5, f64::NAN] {
            let config = RenderLoopConfig {
                present_time_lead_ratio: bad,
                ..RenderLoopConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(ConfigError::PresentLeadRatioOutOfRange(_))),
                "{bad} должен быть отвергнут"
            );
        }
        for good in [0.0, 0.5, 0.99] {
            let config = RenderLoopConfig {
                present_time_lead_ratio: good,
                ..RenderLoopConfig::default()
            };
            assert_eq!(config.validate(), Ok(()), "{good} допустим");
        }
        assert_eq!(RenderLoopConfig::default().present_time_lead_ratio, 0.5);
    }

    #[test]
    fn zero_recent_presentations_capacity_is_rejected_by_name() {
        let config = RenderLoopConfig {
            recent_presentations_capacity: 0,
            ..RenderLoopConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroCapacity {
                field: "recent_presentations_capacity"
            })
        );
    }

    #[test]
    fn partial_json_fills_missing_fields_from_defaults() {
        let config: RenderLoopConfig = serde_json::from_str(r#"{"frames_in_flight": 3}"#).expect("json");
        assert_eq!(config.frames_in_flight, 3);
        assert_eq!(config.preferred_frame_rate_hz, 60.0);
    }

    #[test]
    fn unknown_field_is_rejected() {
        let parsed = serde_json::from_str::<RenderLoopConfig>(r#"{"frames_in_flite": 3}"#);
        assert!(parsed.is_err());
    }

    #[test]
    fn invalid_values_are_rejected_with_specific_errors() {
        let broken = |change: fn(&mut RenderLoopConfig)| {
            let mut config = RenderLoopConfig::default();
            change(&mut config);
            config.validate()
        };
        assert_eq!(broken(|c| c.frames_in_flight = 0), Err(ConfigError::ZeroFramesInFlight));
        assert_eq!(
            broken(|c| c.preferred_frame_rate_hz = 0.0),
            Err(ConfigError::FrameRateOutOfRange(0.0))
        );
        assert!(matches!(
            broken(|c| c.preferred_frame_rate_hz = f32::NAN),
            Err(ConfigError::FrameRateOutOfRange(_))
        ));
        assert_eq!(
            broken(|c| c.acquire_timeout_ms = 0),
            Err(ConfigError::ZeroTimeout {
                field: "acquire_timeout_ms"
            })
        );
        assert_eq!(
            broken(|c| c.pacing_max_miss_ratio = 1.5),
            Err(ConfigError::MissRatioOutOfRange(1.5))
        );
        assert_eq!(
            broken(|c| c.pacing_max_miss_ratio = -0.1),
            Err(ConfigError::MissRatioOutOfRange(-0.1))
        );
        assert!(matches!(
            broken(|c| c.pacing_max_miss_ratio = f64::NAN),
            Err(ConfigError::MissRatioOutOfRange(_))
        ));
        assert_eq!(
            broken(|c| c.pacing_window_s = -1.0),
            Err(ConfigError::WindowOutOfRange(-1.0))
        );
    }

    #[test]
    fn extreme_rate_and_window_that_used_to_panic_in_duration_are_config_errors() {
        // 1e-30 Гц -> период 1e30 с, 1e30 с окна: `Duration::from_secs_f64` паниковал.
        for bad_rate in [1e-30_f32, 0.5, 5000.0, f32::INFINITY, f32::NAN] {
            let config = RenderLoopConfig {
                preferred_frame_rate_hz: bad_rate,
                ..RenderLoopConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(ConfigError::FrameRateOutOfRange(_))),
                "{bad_rate} Гц должен отвергаться"
            );
        }
        for bad_window in [1e30_f64, 3601.0, f64::INFINITY, f64::NAN, 0.0] {
            let config = RenderLoopConfig {
                pacing_window_s: bad_window,
                ..RenderLoopConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(ConfigError::WindowOutOfRange(_))),
                "{bad_window} с должен отвергаться"
            );
        }
        // Границы допустимы, и производные длительности строятся без паники.
        let edge = RenderLoopConfig {
            preferred_frame_rate_hz: MIN_FRAME_RATE_HZ,
            pacing_window_s: MAX_PACING_WINDOW_S,
            ..RenderLoopConfig::default()
        };
        assert_eq!(edge.validate(), Ok(()));
        assert_eq!(edge.expected_refresh_period(), Duration::from_secs(1));
        assert_eq!(edge.pacing_window(), Duration::from_secs(3600));
    }

    #[test]
    fn acquire_plus_teardown_must_fit_the_release_receipt_budget() {
        let defaults = RenderLoopConfig::default();
        assert!(defaults.worst_case_wait_before_release_ms() <= defaults.release_receipt_budget_ms);
        // Бюджет обязан оставаться меньше бюджета оболочки (500 мс по умолчанию).
        assert!(defaults.release_receipt_budget_ms < 500);

        // Пересоздание, начатое до `surfaceDestroyed`, + detach: 2 × 300 = 600 > 450 — это и был
        // остаток ревью 2 (прежнее правило acquire + teardown пропускало такой конфиг).
        let two_teardowns_overflow = RenderLoopConfig {
            teardown_timeout_ms: 300,
            ..RenderLoopConfig::default()
        };
        assert_eq!(
            two_teardowns_overflow.validate(),
            Err(ConfigError::ReleaseBudgetExceeded {
                acquire_timeout_ms: 100,
                teardown_timeout_ms: 300,
                budget_ms: 450,
            })
        );
        let too_slow_acquire = RenderLoopConfig {
            acquire_timeout_ms: 400,
            ..RenderLoopConfig::default()
        };
        assert!(matches!(
            too_slow_acquire.validate(),
            Err(ConfigError::ReleaseBudgetExceeded { .. })
        ));
        // Ровно по границе — можно; переполнение u64 не паникует.
        let exact = RenderLoopConfig {
            acquire_timeout_ms: 225,
            teardown_timeout_ms: 225,
            ..RenderLoopConfig::default()
        };
        assert_eq!(exact.validate(), Ok(()));
        let overflowing = RenderLoopConfig {
            acquire_timeout_ms: u64::MAX,
            teardown_timeout_ms: u64::MAX,
            release_receipt_budget_ms: u64::MAX,
            ..RenderLoopConfig::default()
        };
        // saturating_add: сумма не переполняется (без него debug-сборка паниковала бы).
        assert_eq!(overflowing.validate(), Ok(()));
    }

    #[test]
    fn min_coverage_ratio_must_be_in_half_open_unit_interval() {
        for bad in [0.0, -0.5, 1.01, f64::NAN] {
            let config = RenderLoopConfig {
                pacing_min_coverage_ratio: bad,
                ..RenderLoopConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(ConfigError::CoverageRatioOutOfRange(_))),
                "{bad} должен отвергаться"
            );
        }
        assert_eq!(RenderLoopConfig::default().pacing_min_coverage_ratio, 0.9);
    }

    #[test]
    fn boundary_ratios_are_accepted() {
        for ratio in [0.0, 1.0] {
            let config = RenderLoopConfig {
                pacing_max_miss_ratio: ratio,
                ..RenderLoopConfig::default()
            };
            assert_eq!(config.validate(), Ok(()));
        }
    }

    #[test]
    fn expected_period_follows_preferred_rate() {
        let default_period = RenderLoopConfig::default().expected_refresh_period();
        assert!(
            (default_period.as_secs_f64() - 1.0 / 60.0).abs() < 1e-9,
            "по умолчанию 60 Гц"
        );
        let config = RenderLoopConfig {
            preferred_frame_rate_hz: 120.0,
            ..RenderLoopConfig::default()
        };
        assert!((config.expected_refresh_period().as_secs_f64() - 1.0 / 120.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_present_target_section_fails_the_whole_config() {
        let config = RenderLoopConfig {
            present_target: PresentTargetConfig {
                cost_window_frames: 0,
                ..PresentTargetConfig::default()
            },
            ..RenderLoopConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::PresentTarget(PresentTargetConfigError::WindowOutOfRange {
                field: "cost_window_frames",
                value: 0
            }))
        );
    }

    #[test]
    fn invalid_viewfinder_section_fails_the_whole_config() {
        let config = RenderLoopConfig {
            viewfinder: ViewfinderConfig {
                gpu_time_window_frames: 0,
                ..ViewfinderConfig::default()
            },
            ..RenderLoopConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::Viewfinder(ViewfinderConfigError::GpuTimeWindowOutOfRange(
                0
            )))
        );
        let parsed: RenderLoopConfig =
            serde_json::from_str(r#"{"display_color":"srgb_only","viewfinder":{"painter":"debug_mosaic"}}"#)
                .expect("секции читаются");
        assert_eq!(parsed.display_color, DisplayColorConfig::SrgbOnly);
        assert_eq!(
            parsed.viewfinder.painter,
            crate::viewfinder::ViewfinderPainter::DebugMosaic
        );
    }
}
