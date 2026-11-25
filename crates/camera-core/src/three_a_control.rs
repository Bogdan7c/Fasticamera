//! Контракт автоматики 3A внутри движка (сессия 08): сменные исполнители фокуса и экспозиции.
//!
//! Движок владеет **намерением** (точка интереса, блокировка, профиль, ручные параметры)
//! и спрашивает исполнителей, как его выполнить на текущей камере. Исполнитель — реализация
//! трейта: [`FocusController`] (в 08 — вендорский AF через HAL), [`ExposureController`]
//! (в 08 — временный вендорский AE, в 08a — наш AE по статистике RAW, решения 63 и 66).
//! Заменить часть — дать движку другую реализацию, планировщик и соседей не трогать.
//!
//! Баланс белого сюда не входит: его владелец — `engine/white` (план white-balance), а движок
//! публикует рендеру указание через `DirectiveSlot<WhiteDirective>`.

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

/// Что исполнитель экспозиции знает о сцене для вспышки (14d): без вспышки, «средний серый» нашего AE.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneForFlash {
    /// Экспозицию ведёт не наш AE (вендорский): свет сцены посчитать нечем.
    NotOwnExposure,
    /// Наш AE, но ни одного принятого замера на этом потоке ещё нет.
    NoMeasurement,
    /// Принятая цель нашего AE на этом потоке: произведение «выдержка × ISO» для среднего серого
    /// при компенсации 0 EV (без EV пользователя), то есть свет сцены без вспышки.
    Measured {
        mid_gray_product: camera_contract::auto_exposure::ExposureProduct,
    },
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

    /// Сколько предложений экспозиции отвергнуто как измеренных на другой камере, чем текущий поток (13b).
    /// Нарастающий счётчик за жизнь исполнителя; в сценарии с одной камерой должен быть 0. По умолчанию
    /// (вендорский AE предложений не принимает) — 0.
    fn foreign_camera_proposals_rejected(&self) -> u64 {
        0
    }

    /// Свет сцены для расчёта вспышки (14d). По умолчанию (вендорский AE) — [`SceneForFlash::NotOwnExposure`].
    fn scene_for_flash(&self) -> SceneForFlash {
        SceneForFlash::NotOwnExposure
    }

    /// Линейные замеры предвспышки новее `seen` (14d): ящик нашего AE, куда пишет рендер. Читать
    /// их может только исполнитель, у которого ящик есть; у вендорского AE — всегда `None`.
    fn preflash_samples_newer_than(
        &self,
        _seen: Option<camera_contract::three_a::DirectiveGeneration>,
    ) -> Option<(
        camera_contract::three_a::DirectiveGeneration,
        camera_contract::flash::PreflashSamples,
    )> {
        None
    }

    /// Наблюдение «точка тапа устарела» новее `seen` (решение 241): ящик нашего AE, куда пишет рендер. У вендорского
    /// AE ящика нет — наблюдений не бывает (ограничение: точку снимает только возврат из фона).
    fn point_obsolete_newer_than(
        &self,
        _seen: Option<camera_contract::three_a::DirectiveGeneration>,
    ) -> Option<(
        camera_contract::three_a::DirectiveGeneration,
        camera_contract::scene_change::PointObsolete,
    )> {
        None
    }

    /// Поток или камера сменились: забыть всё, что относится к прошлому потоку.
    fn reset_for_new_stream(&mut self);

    /// Новая камера открыта: то, что исполнитель помнит от прошлого потока в произведении «выдержка
    /// × ISO» (стартовая цель), пересчитать для неё (решения 118, 126). По умолчанию помнить нечего.
    fn carry_memory_to_new_camera(&mut self, _transfer: &crate::exposure_transfer::ExposureTransfer) {}

    /// Поток начинается на камере `camera` (при каждой настройке сессии и повышении тёплой, после
    /// [`Self::reset_for_new_stream`]). Исполнитель, чьи входы приходят с других потоков, по ней отсеивает
    /// входы чужой камеры (13b). По умолчанию камера исполнителю не нужна.
    fn begin_stream_on_camera(&mut self, _camera: &camera_contract::ids::CameraId) {}

    /// Какое произведение «выдержка × ISO» исполнитель ожидает от нового потока для среднего серого, пока
    /// у потока нет собственного замера (память прошлого потока, уже пересчитанная на новую камеру).
    /// Движок публикует его рендеру ДО смены потока (13b). `None` — ожидать нечего (первый поток, вендорский AE).
    fn carried_scene_target(&self) -> Option<camera_contract::auto_exposure::ExposureProduct> {
        None
    }

    /// То же ожидание, но заранее и без изменения состояния: каким оно станет на камере, до которой
    /// `transfer` пересчитывает память. Движок публикует его рендеру ДО обмена тёплой камеры, когда прежний
    /// поток ещё не закрыт и память не перенесена (13b).
    fn scene_target_after_transfer(
        &self,
        _transfer: &crate::exposure_transfer::ExposureTransfer,
    ) -> Option<camera_contract::auto_exposure::ExposureProduct> {
        None
    }
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
