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

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::viewfinder_session::SessionSummary;
use super::white_balance_report::WhiteBalanceReport;

/// Шаг a: вспомогательный поток и первые состояния автоматики HAL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveReport {
    pub companion: CompanionStreamStatus,
    pub af_state: Option<AfState>,
    pub ae_state: Option<AeState>,
}

/// Сравнение нашей белой с белой HAL: справка, не провал.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AwbVsHal {
    /// Разница в пределах порога.
    Ok,
    /// Разница больше `awb.warn_mired_vs_hal`.
    Warn,
    /// Белой HAL или нашей нет.
    Unknown,
}

/// Шаг b: готовность нашего AWB.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AwbReport {
    /// Первая оценка света есть и холодный старт кончился.
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
    pub ae_converged: bool,
    /// Можно ли проверить точку: ожидаемая энергия внутри пределов сенсора камеры.
    pub verifiable: bool,
    /// Отношение медианы к опорной (bias 0).
    pub ratio_to_baseline: Option<f64>,
}

/// Шаг e: компенсация экспозиции.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvReport {
    pub points: Vec<EvPointReport>,
    /// Хотя бы одну точку нельзя проверить из-за пределов сенсора (не провал).
    pub ev_unverifiable_at_limit: bool,
}

/// Шаг f: блокировка AE/AF.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LockReport {
    /// `AE_STATE = Locked` достигнут.
    pub locked: bool,
    pub samples_held: usize,
    /// Результатов под блокировкой, где AE_STATE не Locked.
    pub non_locked_after_lock: usize,
    /// Наибольшее отклонение «выдержка × ISO» от первого кадра под блокировкой, доля.
    pub max_product_deviation: Option<f64>,
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
        ]
    }
}

/// AWB в сводке: без полного отчёта (он в `detail_three_a`), но с белой, HAL-разницей и скрытыми кадрами.
fn compact_awb(awb: &AwbReport) -> Value {
    let white = awb.white_balance.as_ref();
    serde_json::json!({
        "ready": awb.ready,
        "hidden": awb.hidden_since_start,
        "forced": awb.forced_since_start,
        "max_hidden": awb.max_hidden_frames,
        "awb_vs_hal": awb.awb_vs_hal,
        "our_white": white.and_then(|report| report.our_white),
        "confidence": white.and_then(|report| report.confidence),
        "tiles": white.and_then(|report| report.accepted_tiles),
        "hal_delta_mired": white.and_then(|report| report.hal_delta_mired),
        "hal_delta_duv": white.and_then(|report| report.hal_delta_duv),
    })
}
