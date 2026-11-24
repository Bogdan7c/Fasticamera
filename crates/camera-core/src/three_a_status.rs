//! Что движок сообщает наружу об автоматике 3A и ручных параметрах (сессия 08).
//!
//! Снимок [`ThreeAStatus`] — часть [`crate::CameraStatus`]: всегда актуальная правда для
//! экрана (12, 15) и smoke `3a`. Разовые итоги (фокус сошёлся, серия снята, значение
//! ограничено) дополнительно приходят событиями [`crate::CameraEvent`].

use std::time::Duration;

use crate::flash_status::FlashStatus;
use crate::white::WhiteStatus;
use camera_contract::capture_result::{FrameNumber, SensorTimestamp};
use camera_contract::control::ManualControl;
use camera_contract::three_a::{
    AeAfLock, AeState, AfState, BurstId, PointOfInterest, PointRequestId, RequestOrigin, SceneFlicker, ThreeAProfile,
};
use serde::Serialize;

/// Что сейчас делает фокус (с точки зрения намерения, а не сырого `AF_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FocusProgress {
    /// Камера с фиксированным фокусом: управлять нечем.
    FixedLens,
    /// Ручная дистанция.
    Manual,
    /// Непрерывный AF по кадру; `af_state` — последнее, что сообщил HAL (`None` — не сообщил).
    Continuous { af_state: Option<AfState> },
    /// Тап отправлен, скан идёт.
    Scanning { request: PointRequestId },
    /// Итог тапа (держится, пока не придёт новый тап или сброс).
    Settled {
        request: PointRequestId,
        outcome: FocusOutcome,
    },
}

/// Итог однократного фокуса по тапу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusOutcome {
    /// `FOCUSED_LOCKED`.
    Focused,
    /// `NOT_FOCUSED_LOCKED`: HAL честно не нашёл фокус.
    NotFocused,
    /// HAL не сообщил итог за время из конфига (`three_a.focus_settle_timeout_ms`).
    TimedOut,
    /// Разовый запрос с триггером не принят бэкендом — скан не начинался. Не путать с
    /// `TimedOut`: тут HAL тап вообще не получил (новый тап — новая попытка).
    TriggerNotDelivered,
}

/// Какой ручной параметр поправлен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlParameter {
    /// Единицы значений — ISO.
    Iso,
    /// Единицы — наносекунды.
    Shutter,
    /// Единицы — диоптрии.
    Focus,
    /// Единицы — EV.
    ExposureBias,
    /// Единицы — кельвины (температура ручного баланса).
    WhiteBalanceTemperature,
    /// Единицы — Duv (оттенок белой: расстояние от локуса Планка).
    WhiteBalanceTint,
    /// Цель нашего AE, EV **относительно цели**: `requested` всегда 0, `applied` — округлённая
    /// нехватка (< 0 темнее нужного, > 0 светлее). Не ручной параметр пользователя, а честный
    /// итог «пределов камеры не хватило».
    ExposureTarget,
}

/// Почему значение применено не таким, как просили. Молчаливого обрезания нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjustmentReason {
    /// Меньше минимума камеры — поднято до минимума.
    BelowCameraRange,
    /// Больше максимума камеры — опущено до максимума.
    AboveCameraRange,
    /// Выдержка длиннее длительности кадра потока (60 к/с → ≤ 16,67 мс).
    LimitedByFrameDuration,
    /// Округлено до шага HAL (компенсация вендорского AE — 1/6 EV).
    QuantizedToStep,
    /// Камера параметр не поддерживает (например, фокус на фронтальной) — проигнорирован.
    NotSupportedByCamera,
    /// Компенсация при полностью ручной экспозиции не действует.
    IgnoredWhileExposureFullyManual,
    /// Наш AE не смог набрать нужное произведение «выдержка × ISO»: упёрся в диапазон
    /// сенсора, длительность кадра или кратность периоду мерцания. `applied - requested` —
    /// честная нехватка в EV (< 0 темнее нужного, > 0 светлее).
    TargetNotReached,
    /// Белая ниже предела из конфига (`three_a.white`: температура или оттенок) — поднята до предела.
    BelowWhiteLimit,
    /// Белая выше предела из конфига — опущена до предела.
    AboveWhiteLimit,
}

/// Где наш AE на пути к цели (для экрана и smoke); `None` в [`ThreeAStatus::auto_exposure`] —
/// экспозицию ведёт вендорский AE и сказать нечего.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AutoExposureProgress {
    /// Замера от потока рендера ещё не было (идёт стартовое значение).
    AwaitingMeasurement,
    /// Цель ещё движется к оценке сцены.
    Searching,
    /// Цель совпала с оценкой сцены.
    Settled,
    /// Блокировка: цель заморожена.
    Holding,
    /// Пределов камеры не хватает: `shortfall_ev < 0` — темнее нужного, `> 0` — светлее.
    LimitReached { shortfall_ev: f32 },
    /// Обе величины ручные: замер ничего не решает.
    Manual,
}

/// Явный итог «значение ограничено до …» (критерий плана 08).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ControlAdjustment {
    pub parameter: ControlParameter,
    pub requested: f64,
    pub applied: f64,
    pub reason: AdjustmentReason,
}

/// Последний результат HAL с полями автоматики — для экрана и smoke.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LastThreeAResult {
    pub frame_number: FrameNumber,
    pub sensor_timestamp: Option<SensorTimestamp>,
    pub origin: RequestOrigin,
    pub af_state: Option<AfState>,
    pub ae_state: Option<AeState>,
    pub focus_distance_diopters: Option<f32>,
    /// Сырой ответ детектора мерцания HAL на этом кадре (`None` — HAL ключ не отдал). Только
    /// наблюдение: решение о защите принимает исполнитель экспозиции, а не читатели статуса.
    pub scene_flicker: Option<SceneFlicker>,
    pub exposure_time: Option<Duration>,
    pub sensitivity_iso: Option<u32>,
}

/// Счётчики автоматики.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ThreeACounters {
    pub focus_triggers_sent: u64,
    pub focus_trigger_failures: u64,
    pub focus_timeouts: u64,
    pub brackets_submitted: u64,
    pub brackets_completed: u64,
    pub bracket_failures: u64,
    /// Результаты, чей запрос бэкенд не опознал (`RequestOrigin::Unknown`).
    pub results_with_unknown_origin: u64,
    /// Результатов кадров всего (любого происхождения: поток, тап, серия, неопознанные).
    pub results_observed: u64,
    /// Результатов, где HAL сообщил **активный** AE (`AE_STATE` есть и не `Inactive`). Считается по
    /// каждому результату движка, а не по выборке читателей статуса: с нашим AE (`AE_MODE_OFF`) HAL
    /// своего AE не ведёт, и любой такой результат — доказательство, что вендорский AE включился.
    pub results_with_hal_ae_active: u64,
    /// Предложений нашего AE, отвергнутых как измеренных на другой камере, чем текущий поток (13b). Заполняется
    /// при снятии статуса из исполнителя экспозиции; в сценарии с одной камерой должно быть 0.
    pub ae_foreign_camera_proposals_rejected: u64,
}

/// Снимок автоматики 3A и ручных параметров.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreeAStatus {
    pub point: PointOfInterest,
    pub lock: AeAfLock,
    pub profile: ThreeAProfile,
    pub focus: FocusProgress,
    /// Состояние нашего AE; `None`, пока экспозицию ведёт вендорский AE.
    pub auto_exposure: Option<AutoExposureProgress>,
    /// Желаемые ручные параметры (как их задал пользователь).
    pub controls: ManualControl,
    /// Поправки к ним на текущей камере и потоке (пусто — применены как есть).
    pub adjustments: Vec<ControlAdjustment>,
    pub last_result: Option<LastThreeAResult>,
    pub counters: ThreeACounters,
    /// Вспышка активной камеры, совет для Auto и последовательность снимка со вспышкой (14d).
    pub flash: FlashStatus,
    /// Белая: действующее значение и происхождение, заявка источнику, память (собирает `CameraEngine::status`).
    pub white: WhiteStatus,
}

impl ThreeAStatus {
    /// Снимок до первого публикуемого состояния движка: всё на автомате, фокус непрерывный.
    pub fn initial() -> Self {
        Self {
            point: PointOfInterest::WholeFrame,
            lock: AeAfLock::Unlocked,
            profile: ThreeAProfile::Photo,
            focus: FocusProgress::Continuous { af_state: None },
            auto_exposure: None,
            controls: ManualControl::AUTOMATIC,
            adjustments: Vec::new(),
            last_result: None,
            counters: ThreeACounters::default(),
            flash: FlashStatus::default(),
            white: WhiteStatus::initial(),
        }
    }
}

/// Результат одного кадра серии с брекетингом.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BracketFrameResult {
    pub index: u32,
    pub frame_number: FrameNumber,
    pub sensor_timestamp: Option<SensorTimestamp>,
    pub exposure_time: Option<Duration>,
    pub sensitivity_iso: Option<u32>,
}

/// Почему серия не снята целиком.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BracketFailure {
    /// Бэкенд отверг отправку серии.
    SubmitFailed,
    /// Не все кадры серии вернули результат за время из конфига.
    ResultsTimedOut,
    /// Сессия закрылась или камера пропала посреди серии.
    SessionLost,
}

/// Итог серии `burst` (для события).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BracketReport {
    Captured {
        burst: BurstId,
        frames: Vec<BracketFrameResult>,
    },
    Failed {
        burst: BurstId,
        failure: BracketFailure,
        frames: Vec<BracketFrameResult>,
    },
}
