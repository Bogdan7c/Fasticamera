//! Контракт автоматики 3A внутри движка (сессия 08): сменные исполнители фокуса и экспозиции.
//!
//! Движок владеет **намерением** (точка интереса, блокировка, профиль, ручные параметры)
//! и спрашивает исполнителей, как его выполнить на текущей камере. Исполнитель — реализация
//! трейта: [`FocusController`] (в 08 — вендорский AF через HAL), [`ExposureController`]
//! (в 08 — временный вендорский AE, в 08a — наш AE по статистике RAW, решения 63 и 66).
//! Заменить часть — дать движку другую реализацию, планировщик и соседей не трогать.
//!
//! Баланс белого сюда не входит: его исполняет AWB на потоке рендера (решение 64), а движок
//! только публикует режим через `DirectiveSlot<WhiteBalanceDirective>`.

use camera_contract::auto_exposure::ProductReach;
use camera_contract::capabilities::{CameraCapabilities, FrameDuration};
use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::control::{CaptureIntent, ManualControl};
use camera_contract::three_a::{AeAfLock, PointOfInterest, PointRequestId, ThreeAProfile};

use crate::command::UnsupportedThreeA;
use crate::request_controls::{AfTrigger, ExposurePlan, FocusPlan};
use crate::three_a_status::{AutoExposureProgress, ControlAdjustment, FocusOutcome, FocusProgress};

/// Намерение автоматики, которым владеет движок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThreeAIntent {
    pub point: PointOfInterest,
    pub lock: AeAfLock,
    pub profile: ThreeAProfile,
    pub controls: ManualControl,
}

impl ThreeAIntent {
    /// Новый поток: всё на автомате, замер и фокус по кадру.
    pub const AUTOMATIC: ThreeAIntent = ThreeAIntent {
        point: PointOfInterest::WholeFrame,
        lock: AeAfLock::Unlocked,
        profile: ThreeAProfile::Photo,
        controls: ManualControl::AUTOMATIC,
    };
}

impl Default for ThreeAIntent {
    /// Новый поток начинается с автоматики ([`ThreeAIntent::AUTOMATIC`]).
    fn default() -> Self {
        Self::AUTOMATIC
    }
}

/// Всё, что исполнитель знает о камере и потоке при планировании.
#[derive(Debug, Clone, Copy)]
pub struct ThreeAContext<'a> {
    pub intent: &'a ThreeAIntent,
    pub capture_intent: CaptureIntent,
    pub capabilities: &'a CameraCapabilities,
    /// Целевая длительность кадра потока (60 к/с → 16,67 мс): предел ручной выдержки.
    pub stream_frame_duration: FrameDuration,
}

/// План экспозиции с поправками ручных значений под камеру и поток.
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureDecision {
    pub plan: ExposurePlan,
    pub adjustments: Vec<ControlAdjustment>,
    /// Какие произведения «выдержка × ISO» этот исполнитель вообще может поставить в запрос при
    /// действующем намерении; рендер по нему ограничивает остаточную подтяжку. Исполнители, не
    /// отдающие произведение (вендорский AE, ручная экспозиция), ставят `Unbounded`.
    pub reach: ProductReach,
}

/// Изменился ли план без команды владельца.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanRefresh {
    /// План тот же — запрос не перестраивать.
    Unchanged,
    /// План изменился (наш AE в 08a получил новое решение) — движок перестроит
    /// повторяющийся запрос через [`ExposureController::plan`].
    Changed,
}

/// Исполнитель экспозиции.
pub trait ExposureController: Send {
    /// Как выполнить намерение на этой камере. `Err` — честное «не умею» (полуавтомат у
    /// вендорского AE до 08a), желаемое состояние при этом движок не меняет.
    fn plan(&mut self, context: &ThreeAContext<'_>) -> Result<ExposureDecision, UnsupportedThreeA>;

    /// Результат кадра — для исполнителей, которым он нужен (вендорскому AE — нет).
    fn observe_result(&mut self, result: &CaptureResultMeta);

    /// Проверить, не пришло ли новое решение со стороны (наш AE на потоке рендера в 08a). Движок
    /// вызывает после пробуждения [`crate::ThreeAWaker`]; пробуждение идёт по
    /// невытесняемой полосе ящика, а не по полосе кадров.
    fn refresh(&mut self) -> PlanRefresh;

    /// Где исполнитель на пути к цели — для снимка статуса. `None` (по умолчанию) — исполнителю
    /// сказать нечего (вендорский AE ведёт HAL). Предел камеры и «ручной» режим движок
    /// подмешивает сам из плана и намерения: исполнитель отвечает только про свой замер.
    fn progress(&self) -> Option<AutoExposureProgress> {
        None
    }

    /// Поток или камера сменились: забыть всё, что относится к прошлому потоку.
    fn reset_for_new_stream(&mut self);
}

/// План фокуса: что в повторяющийся запрос и нужен ли разовый триггер.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusDecision {
    pub plan: FocusPlan,
    /// Триггер, который движок отправит разовым запросом для тапа `request`.
    pub trigger: Option<FocusTriggerRequest>,
    pub adjustments: Vec<ControlAdjustment>,
}

/// Разовый запрос автофокуса, который надо отправить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusTriggerRequest {
    pub trigger: AfTrigger,
    pub request: PointRequestId,
}

/// Исполнитель фокуса. Он же ведёт машину «тап → скан → итог», потому что её правила
/// зависят от реализации (вендорский AF читает `AF_STATE`, свой AF читал бы другое).
pub trait FocusController: Send {
    /// План для намерения. Триггер для тапа выдаётся в каждом плане, **пока** движок не
    /// подтвердил отправку через [`FocusController::trigger_submitted`]; после подтверждения
    /// тот же `PointRequestId` триггер больше не получает (повторный reconcile не перезапускает фокус).
    fn plan(&mut self, context: &ThreeAContext<'_>) -> Result<FocusDecision, UnsupportedThreeA>;

    /// Движок отправил триггер или отправка провалилась (`TriggerDelivery::Failed`).
    fn trigger_submitted(&mut self, request: FocusTriggerRequest, delivery: TriggerDelivery);

    /// Результат кадра; возвращает итог тапа, если он только что определился.
    fn observe_result(&mut self, result: &CaptureResultMeta) -> Option<(PointRequestId, FocusOutcome)>;

    /// Время ожидания итога вышло (таймер движка из конфига).
    fn settle_timed_out(&mut self, request: PointRequestId) -> Option<FocusOutcome>;

    /// Проверить, не пришёл ли новый регион фокуса со стороны — задел под **фокус-трекинг**
    /// (слово владельца в 08, решение 69): трекер объекта (вероятно, на GPU, на потоке рендера)
    /// публикует регион в нормированных координатах сенсора с меткой кадра, будит движок
    /// [`crate::ThreeAWaker`], и исполнитель отвечает `Changed`, если план фокуса сдвинулся.
    /// Вендорский AF 08 сам ничего не получает — всегда `Unchanged`.
    fn refresh(&mut self) -> PlanRefresh;

    /// Что сейчас делает фокус — для снимка.
    fn progress(&self) -> FocusProgress;

    /// Поток или камера сменились.
    fn reset_for_new_stream(&mut self);
}

/// Дошёл ли разовый запрос до HAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerDelivery {
    Submitted,
    Failed,
}
