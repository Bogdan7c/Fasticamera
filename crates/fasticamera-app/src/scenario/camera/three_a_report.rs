//! Отчёт сценария `3a` (сессия 08): что наблюдали на каждом шаге. Только данные и сводка для строки
//! результата; правила вердикта — в `three_a_verdict`.
//!
//! Секция шага `None`, пока шаг не выполнен до конца: невыполненное не судится (причина обрыва — в `failure`
//! или в нарушении раннего шага). Полные данные идут строками `detail_three_a`, в результате — сводка.

use camera_android::companion_stream::CompanionStreamStatus;
use camera_contract::three_a::{AeState, AfState};
use camera_core::three_a_status::FocusOutcome;
use serde::Serialize;
use serde_json::Value;
use shell_contract::commands::CommandOutcome;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::three_a_own_ae::{AeSource, OwnAeReport};
use super::viewfinder_session::SessionSummary;
use super::white_balance_report::WhiteBalanceReport;

/// Шаг a: вспомогательный поток и первые состояния автоматики HAL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveReport {
    /// Кто ведёт экспозицию: у нашего AE `AE_STATE` HAL — `Inactive`, и его отсутствие не нарушение.
    pub ae_source: AeSource,
    pub companion: CompanionStreamStatus,
    pub af_state: Option<AfState>,
    pub ae_state: Option<AeState>,
}

/// Сравнение применённой белой с текущей белой HAL: справка, не провал.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AwbVsHal {
    /// Разница в пределах порога.
    Ok,
    /// Разница больше `awb.warn_mired_vs_hal`.
    Warn,
    /// Белой HAL или применённой нет.
    Unknown,
}

/// Шаг b: белая взята и показана.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AwbReport {
    /// Заявка белого закрыта исходом «принято» и холодный старт кончился.
    pub ready: bool,
    /// Скрыто кадров с начала сценария (не за сессию показа).
    pub hidden_since_start: u64,
    /// Принудительных показов с начала сценария (лимит скрытия исчерпан без оценки).
    pub forced_since_start: u64,
    /// Предел скрытых кадров из конфига рендера.
    pub max_hidden_frames: u32,
    pub awb_vs_hal: AwbVsHal,
    pub white_balance: Option<WhiteBalanceReport>,
}

/// Чем кончился тап.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TapResult {
    Focused,
    NotFocused,
    /// Движок не дождался итога HAL.
    TimedOut,
    /// Бэкенд отверг разовый запрос: HAL тап не получил.
    TriggerNotDelivered,
    /// Событие итога не пришло за срок сценария.
    NoEvent,
    /// Движок сам отпустил точку тапа до итога (решение 241: сцена под ней сменилась): тап не проверен.
    ReleasedBySceneChange,
}

impl TapResult {
    pub fn of(outcome: FocusOutcome) -> Self {
        match outcome {
            FocusOutcome::Focused => Self::Focused,
            FocusOutcome::NotFocused => Self::NotFocused,
            FocusOutcome::TimedOut => Self::TimedOut,
            FocusOutcome::TriggerNotDelivered => Self::TriggerNotDelivered,
        }
    }
}

/// Один тап.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TapReport {
    pub x: f32,
    pub y: f32,
    pub result: TapResult,
    /// Кадров от отправки тапа до итога (точность — тик, ~3 кадра).
    pub frames_to_settle: Option<i64>,
    pub diopters_before: Option<f32>,
    pub diopters_after: Option<f32>,
}

/// Шаг c: тапы фокуса.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FocusReport {
    pub taps: Vec<TapReport>,
    /// Разброс дистанций у разных точек (макс − мин, диоптрии): есть ли региональный эффект. Справка.
    pub region_effect_diopters: Option<f32>,
    /// Триггеров отправлено за шаг (счётчик движка).
    pub triggers_sent: u64,
    /// Результаты без контекста запроса за шаг (бэкенд не опознал свой запрос).
    pub results_without_request_context: u64,
    /// Результаты с неопознанным происхождением за шаг (движок).
    pub results_with_unknown_origin: u64,
}

/// Достигнутая экспозиция.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AppliedExposure {
    pub exposure_ns: u64,
    pub iso: u32,
    /// Кадров от команды до результата с этими значениями (точность — тик).
    pub latency_frames: i64,
}

/// Слишком длинная выдержка.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LongShutterReport {
    pub requested_ns: u64,
    pub frame_budget_ns: Option<u64>,
    /// Пришло `ControlsAdjusted` с причиной «ограничено длительностью кадра» для выдержки.
    pub limited_event_seen: bool,
    pub applied_ns: Option<u64>,
}

/// Шаг d: ручная экспозиция.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManualReport {
    pub requested_exposure_ns: u64,
    pub requested_iso: u32,
    pub applied: Option<AppliedExposure>,
    /// События `ControlsAdjusted` на значения внутри диапазона (должно быть 0).
    pub adjustments_for_in_range: usize,
    pub long_shutter: LongShutterReport,
}

/// Точка шага EV.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvPointReport {
    pub bias_ev: f32,
    /// Медиана «выдержка × ISO», нс·ISO.
    pub median_product: Option<f64>,
    pub samples: usize,
    /// AE сошёлся: вендорский — HAL `AE_STATE = Converged`, наш — `Settled`/`Holding` в статусе движка.
    pub ae_converged: bool,
    /// Наш AE упёрся в пределы камеры (`LimitReached`): точку нельзя проверить, это честный итог, а не таймаут.
    pub ae_limit_reached: bool,
    /// Можно ли проверить точку: ожидаемая энергия внутри пределов сенсора камеры.
    pub verifiable: bool,
    /// Отношение медианы к опорной (bias 0).
    pub ratio_to_baseline: Option<f64>,
    /// По какой области мерилась точка (решение 135: шаг сбрасывает тап и мерит по всему кадру).
    pub metered_area: EvMeteredArea,
}

/// Область замера точки EV — из статуса движка на момент записи точки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvMeteredArea {
    /// Весь кадр (матричный замер): так шаг EV меряется с 13c.
    WholeFrame,
    /// Точка тапа: сброс не дошёл до движка за срок — точка мерилась по последнему тапу.
    TapPoint,
}

/// Шаг e: компенсация экспозиции.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvReport {
    /// Кто вёл экспозицию: от этого зависит, как судить сходимость точек.
    pub ae_source: AeSource,
    pub points: Vec<EvPointReport>,
    /// Хотя бы одну точку нельзя проверить из-за пределов сенсора (не провал).
    pub ev_unverifiable_at_limit: bool,
}

/// Шаг f: блокировка AE/AF.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LockReport {
    pub ae_source: AeSource,
    /// Блокировка достигнута: вендорский AE — `AE_STATE = Locked`, наш — `Holding` в статусе движка.
    pub locked: bool,
    pub samples_held: usize,
    /// Под блокировкой: вендорский AE — результатов с `AE_STATE` не `Locked`, наш — тиков, где статус
    /// вышел из `Holding`.
    pub non_locked_after_lock: usize,
    /// Наибольшее отклонение «выдержка × ISO» от первого кадра под блокировкой, доля.
    pub max_product_deviation: Option<f64>,
    /// Размах сырого замера сцены нашим AE за время удержания, EV: доказательство, что сцену во время
    /// блокировки меняли (закрыли объектив рукой). `None` — замера не было (вендорский AE). Около нуля —
    /// блокировка проверена на неподвижной сцене, `max_product_deviation = 0` тогда ничего не доказывает.
    pub observed_scene_range_ev: Option<f64>,
    /// Через сколько кадров после кадра команды блокировки «выдержка × ISO» замерло до конца удержания:
    /// факт, по которому выбирают `lock.own_ae_catch_up_frames`. `None` — не замерло (или кадров нет).
    pub product_frozen_after_frames: Option<i64>,
    /// Итог показа отсчёта «приготовься» перед удержанием (первая непринятая подсказка отсчёта, иначе `queued`);
    /// `None` — отсчёта не было (`lock.prepare_countdown_ms = 0`) или шаг не дошёл до него.
    pub prompt_prepare: Option<CommandOutcome>,
    /// Итог показа подсказки в начале удержания («закрой объектив»); `None` — удержание не началось.
    pub prompt_hold_start: Option<CommandOutcome>,
    /// Итог показа подсказки в конце удержания («убери руку»).
    pub prompt_hold_end: Option<CommandOutcome>,
    /// После сброса точки: непрерывный AF и блокировка снята.
    pub back_to_continuous: bool,
}

/// Кадр серии в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BracketFrameReport {
    pub index: u32,
    pub frame_number: i64,
    pub exposure_ns: Option<u64>,
    pub iso: Option<u32>,
}

/// Шаг g: брекетинг.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BracketRunReport {
    /// Серия снята целиком (`BracketReport::Captured`); `None` — итог не пришёл за срок.
    pub captured: Option<bool>,
    /// Причина провала серии, если она не снята.
    pub failure: Option<String>,
    pub frames: Vec<BracketFrameReport>,
    /// Пропуски в номерах кадров серии (0 — подряд).
    pub frame_gaps: i64,
    /// Прирост `bracket_frames_skipped` рендера.
    pub skipped_delta: u64,
    /// Потери кадров видоискателя за время серии.
    pub dropped_during: u64,
    /// Показано новых кадров камеры за время серии (видоискатель не остановился).
    pub presented_during: u64,
}

/// Всё наблюдение сценария.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreeAObservation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    pub session: SessionSummary,
    pub live: Option<LiveReport>,
    pub awb: Option<AwbReport>,
    pub focus: Option<FocusReport>,
    pub manual: Option<ManualReport>,
    pub ev: Option<EvReport>,
    pub lock: Option<LockReport>,
    pub bracket: Option<BracketRunReport>,
    /// Какой исполнитель экспозиции должен вести камеру по выбору композиции (сверяется с тем, что видно в движке).
    pub expected_ae_source: AeSource,
    /// Почему он не тот, что в конфиге (рендер недоступен); `None` — выбор совпал с конфигом.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ae_fallback_reason: Option<String>,
    /// Наш AE за прогон; `None` — экспозицию вёл вендорский.
    pub own_ae: Option<OwnAeReport>,
    /// Не удалось вернуть автоматические параметры в конце (причина).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_failure: Option<String>,
}

impl ThreeAObservation {
    /// Пустое наблюдение до первого шага.
    pub fn new(session: SessionSummary) -> Self {
        Self {
            failure: None,
            session,
            live: None,
            awb: None,
            focus: None,
            manual: None,
            ev: None,
            lock: None,
            bracket: None,
            expected_ae_source: AeSource::Vendor,
            ae_fallback_reason: None,
            own_ae: None,
            restore_failure: None,
        }
    }

    /// Сводка для строки результата: ключевые числа шагов (журнал событий сессии — не здесь).
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "failure": self.failure,
            "state_at_end": self.session.state_at_end,
            "time_to_live_ms": self.session.time_to_live_ms,
            "sampled_every_tick": "frames_to_settle and latency_frames have tick (~3 frames) precision",
            "live": self.live,
            "awb": self.awb.as_ref().map(compact_awb),
            "focus": self.focus,
            "manual": self.manual,
            "ev": self.ev,
            "lock": self.lock,
            "bracket": self.bracket,
            "expected_ae_source": self.expected_ae_source,
            "ae_fallback_reason": self.ae_fallback_reason,
            "own_ae": self.own_ae,
            "restore_failure": self.restore_failure,
        })
    }

    /// Подробности частями (каждая — элемент `detail_three_a`).
    pub fn detail_items(&self) -> Vec<Value> {
        vec![
            serde_json::json!({ "session": to_value(&self.session) }),
            serde_json::json!({ "awb": to_value(&self.awb) }),
            serde_json::json!({ "focus": to_value(&self.focus), "lock": to_value(&self.lock) }),
            serde_json::json!({ "ev": to_value(&self.ev), "bracket": to_value(&self.bracket) }),
            serde_json::json!({ "own_ae": to_value(&self.own_ae) }),
        ]
    }
}

/// Белая в сводке: без полного отчёта (он в `detail_three_a`), но с белой, происхождением, исходом заявки,
/// HAL-разницей и скрытыми кадрами.
fn compact_awb(awb: &AwbReport) -> Value {
    let white = awb.white_balance.as_ref();
    serde_json::json!({
        "ready": awb.ready,
        "hidden": awb.hidden_since_start,
        "forced": awb.forced_since_start,
        "max_hidden": awb.max_hidden_frames,
        "awb_vs_hal": awb.awb_vs_hal,
        "white": white.map(|report| report.white),
        "origin": white.map(|report| report.origin.as_str()),
        // Исход последней заявки одним словом (номер и источник — в `detail_three_a`): строка результата тесна.
        "request": white.map(|report| report.request.state),
        "hal_delta_mired": white.and_then(|report| report.hal_delta_mired),
        "hal_delta_duv": white.and_then(|report| report.hal_delta_duv),
    })
}
