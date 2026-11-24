//! Владелец белого: чистая машина состояний без потоков, ящиков и знаний о рендере.
//!
//! Владелец хранит **единственную** действующую белую с происхождением и единственную заявку источнику.
//! Меняют их только входы машины: ручная команда, команда «взять у источника», возобновление приложения,
//! уход в фон, кадры потока, конец потока, ответ источника и срок. Время всегда параметр (`Instant`).
//!
//! Инварианты:
//! * без входа белая не меняется ни на бит;
//! * ответ опознаётся ТОЛЬКО номером заявки; номера выдаются по возрастанию и не переиспользуются;
//! * рука сильнее: ручная белая отменяет ожидающую заявку, и опоздавший ответ становится чужим;
//! * срок ожидания открытия считается от первого кадра потока (см. [`WhiteOwner::on_stream_frame`]), срок
//!   команды — от самой команды;
//! * отказ источника (занят, не знаю, нет источника, сломался, срок, непонятный свет) белую не меняет.
//!
//! Всё, что надо донести наружу (событие, запись в память), копится в `notices` и забирается
//! [`WhiteOwner::drain_notices`]: машина сама ничего не публикует.

use std::time::{Duration, Instant};

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::{
    HeldWhite, SceneIlluminant, WhileAwaitingWhite, WhiteAnswer, WhiteDirective, WhiteOrigin, WhiteRequestId,
    WhiteSourceId, WhiteSourceOutcome, WhiteSourceRequest, WhiteTransition,
};

use super::limits::clamp_white;
use crate::three_a_status::ControlAdjustment;
use crate::white::{
    RememberedOrigin, WhiteCancelCause, WhiteConfig, WhiteMemoryLoad, WhiteMemoryStartup, WhiteRequestOutcome,
    WhiteRequestResult, WhiteRequestState,
};

/// Что владелец просит донести наружу (события и память), в порядке появления.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum WhiteNotice {
    /// Действующая белая изменилась (в том числе сменилось только происхождение).
    Changed(HeldWhite),
    /// Названное значение прижато к пределам.
    Adjusted(Vec<ControlAdjustment>),
    /// Заявка закрыта.
    Finished(WhiteRequestResult),
}

/// Итог ручной команды для вызывающего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum ManualOutcome {
    /// Белая (или отмена ожидающей заявки) изменилась.
    Changed,
    /// Та же ручная белая, и отменять нечего.
    AlreadyHeld,
}

/// Заявка, ожидающая ответа.
#[derive(Debug, Clone, Copy)]
struct Pending {
    request: WhiteSourceRequest,
    /// Как переехать к белой из ответа: решает тот, кто завёл заявку (открытие или команда).
    on_ready: WhiteTransition,
    /// Когда срок выходит; `None` — отсчёт ещё не начат (первого кадра потока не было).
    deadline: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
enum Request {
    Idle,
    Waiting(Pending),
}

/// Машина владельца белого.
pub(super) struct WhiteOwner {
    config: WhiteConfig,
    opening_source: WhiteSourceId,
    held: HeldWhite,
    /// Поправки к последнему названному значению.
    limits: Vec<ControlAdjustment>,
    /// Как переехать к `held`: переход последнего изменения (рендер сравнивает белую, а не номер).
    transition: WhiteTransition,
    request: Request,
    /// Чем кончилась последняя заявка.
    last_result: Option<WhiteRequestResult>,
    /// Номер, который получит следующая заявка.
    next_request: u64,
    /// Был ли кадр повторяющегося запроса текущего потока.
    frame_seen: bool,
    stale_answers: u64,
    memory_startup: WhiteMemoryStartup,
    notices: Vec<WhiteNotice>,
}

impl WhiteOwner {
    /// Владелец со стартовой белой: запомненная (прижатая к пределам) или запасной свет.
    /// `first_request` — наименьший номер, который можно выдать (номера не переиспользуются и между запусками
    /// движка над одним ящиком ответов).
    pub(super) fn new(
        config: WhiteConfig,
        opening_source: WhiteSourceId,
        remembered: WhiteMemoryLoad,
        first_request: u64,
    ) -> Self {
        let fallback = HeldWhite {
            setting: config.fallback_setting_or_daylight(),
            origin: WhiteOrigin::Fallback,
        };
        let mut notices = Vec::new();
        let mut limits = Vec::new();
        let (held, memory_startup) = match remembered {
            WhiteMemoryLoad::NothingRemembered => (fallback, WhiteMemoryStartup::NothingRemembered),
            WhiteMemoryLoad::Unreadable { reason } => {
                tracing::warn!(target: super::super::LOG_TARGET, %reason, "память белой не читается, взят запасной свет");
                (fallback, WhiteMemoryStartup::Unreadable { reason })
            }
            WhiteMemoryLoad::Remembered(record) => {
                let clamped = clamp_white(&config, record.setting);
                if !clamped.adjustments.is_empty() {
                    notices.push(WhiteNotice::Adjusted(clamped.adjustments.clone()));
                }
                limits = clamped.adjustments;
                let origin = match record.origin {
                    RememberedOrigin::Source => WhiteOrigin::Source(opening_source),
                    RememberedOrigin::Manual => WhiteOrigin::Manual,
                };
                (
                    HeldWhite {
                        setting: clamped.setting,
                        origin,
                    },
                    WhiteMemoryStartup::Restored,
                )
            }
        };
        Self {
            config,
            opening_source,
            held,
            limits,
            transition: WhiteTransition::Immediate,
            request: Request::Idle,
            last_result: None,
            next_request: first_request.max(1),
            frame_seen: false,
            stale_answers: 0,
            memory_startup,
            notices,
        }
    }

    // ---- Входы машины ----

    /// Рука: выставить белую вручную. Прижим к пределам виден в `limits` и в уведомлении.
    pub(super) fn set_manual(&mut self, requested: WhiteBalanceSetting, transition: WhiteTransition) -> ManualOutcome {
        let clamped = clamp_white(&self.config, requested);
        let same_white = self.held.setting == clamped.setting && self.held.origin == WhiteOrigin::Manual;
        let had_request = matches!(self.request, Request::Waiting(_));
        // Отчёт о прижиме относится к этой просьбе, даже если белая от неё не изменилась.
        self.limits = clamped.adjustments.clone();
        if same_white && !had_request {
            return ManualOutcome::AlreadyHeld;
        }
        // Рука сильнее: ожидающая заявка отменяется, её опоздавший ответ станет чужим.
        self.cancel_request(WhiteCancelCause::ManualWhite);
        if !same_white {
            self.held = HeldWhite {
                setting: clamped.setting,
                origin: WhiteOrigin::Manual,
            };
            self.transition = transition;
            self.notices.push(WhiteNotice::Changed(self.held));
        }
        if !clamped.adjustments.is_empty() {
            self.notices.push(WhiteNotice::Adjusted(clamped.adjustments));
        }
        ManualOutcome::Changed
    }

    /// Команда «взять белую у источника» при видимой картинке (кадры не скрываются).
    pub(super) fn take_from_source(&mut self, source: WhiteSourceId, transition: WhiteTransition, now: Instant) {
        // Срок команды — от нажатия: человек ждёт ответа у экрана, а не первого кадра.
        let deadline = now + self.answer_timeout();
        self.open_request(source, WhileAwaitingWhite::KeepShowing, transition, Some(deadline));
    }

    /// Приложение вернулось на передний план (WB22): ручную белую держим, иначе спрашиваем источник открытия,
    /// пока кадры скрыты.
    pub(super) fn on_foreground(&mut self) {
        if self.held.origin == WhiteOrigin::Manual {
            return;
        }
        let transition = self.late_transition();
        self.open_request(self.opening_source, WhileAwaitingWhite::HideFrames, transition, None);
    }

    /// Приложение ушло в фон: ожидание снимается, белая остаётся.
    pub(super) fn on_background(&mut self) {
        self.cancel_request(WhiteCancelCause::Background);
    }

    /// Пришёл результат повторяющегося запроса: поток жив. Первый из них запускает срок ожидания.
    pub(super) fn on_stream_frame(&mut self, now: Instant) {
        self.frame_seen = true;
        let timeout = self.answer_timeout();
        if let Request::Waiting(pending) = &mut self.request
            && pending.deadline.is_none()
        {
            pending.deadline = Some(now + timeout);
        }
    }

    /// Поток кончился (сессия закрыта, камера потеряна, запрос остановлен).
    ///
    /// Заявка, которой поток уже служил, переиздаётся с новым номером: ответ прежнего потока станет чужим, вид
    /// ожидания тот же. Срок открытия снят до первого кадра нового потока; срок команды идёт дальше — иначе
    /// команда без нового потока (камера закрыта, отказ) висела бы вечно и сработала бы при следующем открытии
    /// поверх ручной белой. Если кадров с выдачи заявки не было, ответа
    /// прежнего потока быть не может и заявка остаётся как есть (без лишних номеров на каждой попытке открытия).
    pub(super) fn on_stream_ended(&mut self) {
        let had_frames = std::mem::replace(&mut self.frame_seen, false);
        let Request::Waiting(pending) = self.request else {
            return;
        };
        if !had_frames {
            return;
        }
        self.request = Request::Waiting(Pending {
            request: WhiteSourceRequest {
                id: self.issue_id(),
                ..pending.request
            },
            deadline: match pending.request.while_awaiting {
                WhileAwaitingWhite::HideFrames => None,
                WhileAwaitingWhite::KeepShowing => pending.deadline,
            },
            ..pending
        });
    }

    /// Ответ источника.
    pub(super) fn on_answer(&mut self, answer: &WhiteAnswer) {
        let Request::Waiting(pending) = self.request else {
            self.stale_answers += 1;
            return;
        };
        if pending.request.id != answer.request {
            self.stale_answers += 1;
            return;
        }
        let outcome = match answer.outcome {
            WhiteSourceOutcome::Ready {
                illuminant: SceneIlluminant::Uniform(setting),
                confidence,
            } => {
                self.apply_source_white(pending.request.source, setting, pending.on_ready);
                WhiteRequestOutcome::Applied { confidence }
            }
            // Свет новее владельца: честно «не понял», белая прежняя.
            WhiteSourceOutcome::Ready { .. } => WhiteRequestOutcome::NotUnderstood,
            WhiteSourceOutcome::Busy => WhiteRequestOutcome::Busy,
            WhiteSourceOutcome::Unknown { reason } => WhiteRequestOutcome::Unknown { reason },
            WhiteSourceOutcome::Unavailable { reason } => WhiteRequestOutcome::Unavailable { reason },
            WhiteSourceOutcome::Failed { failure } => WhiteRequestOutcome::Failed { failure },
        };
        self.finish_request(pending, outcome);
    }

    /// Сработал таймер: срок ожидания мог выйти.
    pub(super) fn on_deadline(&mut self, now: Instant) {
        let Request::Waiting(pending) = self.request else {
            return;
        };
        if pending.deadline.is_some_and(|deadline| now >= deadline) {
            self.finish_request(pending, WhiteRequestOutcome::TimedOut);
        }
    }

    // ---- Выходы машины ----

    /// Когда машине надо проснуться без внешних событий.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        match self.request {
            Request::Waiting(pending) => pending.deadline,
            Request::Idle => None,
        }
    }

    /// Указание рендеру: какая белая, как к ней переехать и какая заявка действует.
    pub(super) fn directive(&self) -> WhiteDirective {
        WhiteDirective {
            white: self.held.setting,
            transition: self.transition,
            request: match self.request {
                Request::Waiting(pending) => Some(pending.request),
                Request::Idle => None,
            },
        }
    }

    pub(super) fn held(&self) -> HeldWhite {
        self.held
    }

    pub(super) fn limits(&self) -> &[ControlAdjustment] {
        &self.limits
    }

    pub(super) fn request_state(&self) -> WhiteRequestState {
        match self.request {
            Request::Idle => WhiteRequestState::Idle { last: self.last_result },
            Request::Waiting(pending) => WhiteRequestState::Waiting {
                request: pending.request.id,
                source: pending.request.source,
            },
        }
    }

    pub(super) fn stale_answers(&self) -> u64 {
        self.stale_answers
    }

    pub(super) fn memory_startup(&self) -> &WhiteMemoryStartup {
        &self.memory_startup
    }

    /// Забрать накопленные уведомления.
    pub(super) fn drain_notices(&mut self) -> Vec<WhiteNotice> {
        std::mem::take(&mut self.notices)
    }

    // ---- Внутреннее ----

    fn answer_timeout(&self) -> Duration {
        Duration::from_millis(self.config.answer_timeout_ms)
    }

    /// Переход для ответа, пришедшего после открытия (WB23): ноль в конфиге — сразу.
    fn late_transition(&self) -> WhiteTransition {
        match self.config.late_transition_ms {
            0 => WhiteTransition::Immediate,
            millis => WhiteTransition::Over(Duration::from_millis(millis)),
        }
    }

    fn issue_id(&mut self) -> WhiteRequestId {
        let id = WhiteRequestId(self.next_request);
        self.next_request += 1;
        id
    }

    /// Завести заявку; прежняя ожидающая закрывается как замещённая. `deadline: None` — срок начнёт первый кадр потока.
    fn open_request(
        &mut self,
        source: WhiteSourceId,
        while_awaiting: WhileAwaitingWhite,
        on_ready: WhiteTransition,
        deadline: Option<Instant>,
    ) {
        self.cancel_request(WhiteCancelCause::ReplacedByNewRequest);
        let id = self.issue_id();
        self.request = Request::Waiting(Pending {
            request: WhiteSourceRequest {
                id,
                source,
                while_awaiting,
            },
            on_ready,
            deadline,
        });
    }

    fn cancel_request(&mut self, by: WhiteCancelCause) {
        if let Request::Waiting(pending) = self.request {
            self.finish_request(pending, WhiteRequestOutcome::Cancelled { by });
        }
    }

    fn finish_request(&mut self, pending: Pending, result: WhiteRequestOutcome) {
        let finished = WhiteRequestResult {
            request: pending.request.id,
            source: pending.request.source,
            result,
        };
        self.request = Request::Idle;
        self.last_result = Some(finished);
        self.notices.push(WhiteNotice::Finished(finished));
    }

    /// Принять белую источника: прижать к пределам, поставить происхождение «источник».
    fn apply_source_white(&mut self, source: WhiteSourceId, setting: WhiteBalanceSetting, transition: WhiteTransition) {
        let clamped = clamp_white(&self.config, setting);
        let new_held = HeldWhite {
            setting: clamped.setting,
            origin: WhiteOrigin::Source(source),
        };
        self.limits = clamped.adjustments.clone();
        if new_held != self.held {
            self.held = new_held;
            self.transition = transition;
            self.notices.push(WhiteNotice::Changed(new_held));
        }
        if !clamped.adjustments.is_empty() {
            self.notices.push(WhiteNotice::Adjusted(clamped.adjustments));
        }
    }
}
