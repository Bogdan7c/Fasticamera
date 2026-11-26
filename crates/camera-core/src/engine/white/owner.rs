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
//! * отказ источника (занят, не знаю, нет источника, сломался, срок, непонятный свет) белую не меняет;
//! * вопросы слежения ([`RequestPurpose::FollowAsk`]) внутренние: они не перебивают чужую заявку, не видны в
//!   состоянии заявки, итогах и отчёте команды, снимаются молча; без включённого слежения их нет вовсе;
//! * любое действие человека с белой (рука, принятая команда «взять») выключает слежение;
//! * правило возобновления ([`WhiteResumeRule`]) читает ТОЛЬКО [`WhiteOwner::on_foreground`]: другого места,
//!   где белая решает «спросить источник или держать», нет. Смена правила белую, переход и ожидающую заявку
//!   не трогает и действует со следующего возобновления.
//!
//! Всё, что надо донести наружу (событие, запись в память), копится в `notices` и забирается
//! [`WhiteOwner::drain_notices`]: машина сама ничего не публикует.

use std::time::{Duration, Instant};

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::WhiteBalanceSetting;
use camera_contract::three_a::AeAfLock;
use camera_contract::white::{
    HeldWhite, SceneIlluminant, WhileAwaitingWhite, WhiteAdjustment, WhiteAnswer, WhiteDirective, WhiteOrigin,
    WhiteRequestId, WhiteSourceId, WhiteSourceOutcome, WhiteSourceRequest, WhiteTransition,
};

use super::follow::{FollowChange, FollowTick, FollowVerdict, WhiteFollow};
use super::limits::clamp_white;
use crate::command::{CommandId, InvalidStateReason};
use crate::three_a_status::ControlAdjustment;
use crate::white::{
    RememberedOrigin, ResumeRuleChoice, WhiteCancelCause, WhiteConfig, WhiteFollowPause, WhiteFollowRecord,
    WhiteFollowSpeed, WhiteMemoryLoad, WhiteMemoryStartup, WhiteMemoryState, WhiteRequestOutcome, WhiteRequestResult,
    WhiteRequestState, WhiteResumeRule, WhiteStatus, WhiteTakeReport,
};

/// Что владелец просит донести наружу (события и память), в порядке появления.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum WhiteNotice {
    /// Действующая белая изменилась (в том числе сменилось только происхождение).
    Changed(HeldWhite),
    /// Белую сдвинуло слежение. Отдельно от [`Self::Changed`]: таких смен бывает несколько в секунду, и память их
    /// запоминает не каждую (решает [`super::WhiteEngine`]).
    Followed(HeldWhite),
    /// Названное значение прижато к пределам.
    Adjusted(Vec<ControlAdjustment>),
    /// Заявка закрыта.
    Finished(WhiteRequestResult),
    /// Человек выбрал правило возобновления (запомнить и сообщить).
    ResumeRuleChosen(WhiteResumeRule),
    /// Слежение включено, выключено или сменило скорость (запомнить и сообщить).
    FollowChanged(WhiteFollowRecord),
}

/// Что владелец получает из памяти при старте.
#[derive(Debug, Clone)]
pub(super) struct WhiteRemembered {
    pub(super) white: WhiteMemoryLoad,
    /// Выбор правила, сохранённый памятью: он сильнее конфига и новых записей в память не порождает.
    pub(super) resume_rule: Option<WhiteResumeRule>,
    pub(super) follow: WhiteFollowRecord,
}

/// Тронул ли вход машины заявку (и надо ли публиковать указание).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameEffect {
    Nothing,
    RequestChanged,
}

/// Зачем заведена заявка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestPurpose {
    /// Возобновление приложения.
    Opening,
    /// Команда «взять белую». Переживает переиздание заявки при конце потока: отчёт команды остаётся «ждёт».
    Take(CommandId),
    /// Очередной вопрос слежения: срок у него по меткам кадров, а не по часам.
    FollowAsk,
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
    purpose: RequestPurpose,
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
    /// Правило, которое выбрал человек (в этом запуске или раньше — из памяти). `None` — выбора нет, действует
    /// умолчание конфига: см. [`Self::resume_rule`].
    chosen_rule: Option<WhiteResumeRule>,
    follow: WhiteFollow,
    held: HeldWhite,
    /// Поправки к последнему названному значению.
    limits: Vec<ControlAdjustment>,
    /// Как переехать к `held`: переход последнего изменения (рендер сравнивает белую, а не номер).
    transition: WhiteTransition,
    request: Request,
    /// Чем кончилась последняя заявка.
    last_result: Option<WhiteRequestResult>,
    /// Отчёт о последней команде «взять белую». Заявка открытия его не трогает.
    last_take: Option<WhiteTakeReport>,
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
        remembered: WhiteRemembered,
        first_request: u64,
    ) -> Self {
        let fallback = HeldWhite {
            setting: config.fallback_setting_or_daylight(),
            origin: WhiteOrigin::Fallback,
        };
        let mut notices = Vec::new();
        let mut limits = Vec::new();
        let (held, memory_startup) = match remembered.white {
            WhiteMemoryLoad::NothingRemembered => (fallback, WhiteMemoryStartup::NothingRemembered),
            WhiteMemoryLoad::Unreadable { reason } => {
                // Причину в журнал пишет `WhiteEngine::new`: владельца собирают и ради снимка до движка.
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
            chosen_rule: remembered.resume_rule,
            follow: WhiteFollow::restored(config.follow, remembered.follow, opening_source),
            held,
            limits,
            transition: WhiteTransition::Immediate,
            request: Request::Idle,
            last_result: None,
            last_take: None,
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
        // Рука выключает слежение, даже если назвала ту же белую.
        let follow_stopped = self.stop_follow() == FollowChange::Changed;
        if same_white && !had_request && !follow_stopped {
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

    /// Рука, одна ось: вторая берётся у ДЕЙСТВУЮЩЕЙ белой владельца в этот момент (а не у экрана, чьи факты
    /// могли отстать на кадр: например, ответ источника только что сменил белую). Нетронутая ось остаётся
    /// бит-в-бит; дальше всё как у [`Self::set_manual`].
    pub(super) fn adjust_manual(&mut self, adjustment: WhiteAdjustment, transition: WhiteTransition) -> ManualOutcome {
        let requested = adjustment.applied_to(self.held.setting);
        self.set_manual(requested, transition)
    }

    /// Команда «взять белую у источника» при видимой картинке (кадры не скрываются). Заменяет и заявку, и
    /// отчёт прежней команды: прежняя заявка закрывается как замещённая, её отчёт — вместе с ней.
    pub(super) fn take_from_source(
        &mut self,
        command: CommandId,
        source: WhiteSourceId,
        transition: WhiteTransition,
        now: Instant,
    ) {
        // Взять разово — значит дальше держать: слежение выключается, чем бы заявка ни кончилась.
        self.stop_follow();
        // Срок команды — от нажатия: человек ждёт ответа у экрана, а не первого кадра.
        let deadline = now + self.answer_timeout();
        self.open_request(
            source,
            WhileAwaitingWhite::KeepShowing,
            transition,
            Some(deadline),
            RequestPurpose::Take(command),
        );
    }

    /// Команда «взять белую» отклонена до заявки (нет потока, движок сломан): источнику ничего не передавали.
    /// Отчёт заменяется; заявка более ранней команды, если ещё ждёт, остаётся и закроется как обычно (её отчёт
    /// к тому времени уже не последний и не перезаписывается: см. [`Self::finish_request`]).
    pub(super) fn note_take_not_asked(&mut self, command: CommandId, reason: InvalidStateReason) {
        self.last_take = Some(WhiteTakeReport::not_asked(command, reason));
    }

    /// Следить за источником. Ждущая команда «взять» замещается (человек передумал); заявка открытия остаётся: она
    /// прячет кадры до верного цвета, и её ответ — тот же свет.
    pub(super) fn follow_source(&mut self, source: WhiteSourceId) -> FollowChange {
        if self.follow.start(source) == FollowChange::Unchanged {
            return FollowChange::Unchanged;
        }
        if let Request::Waiting(Pending {
            purpose: RequestPurpose::Take(_) | RequestPurpose::FollowAsk,
            ..
        }) = self.request
        {
            self.cancel_request(WhiteCancelCause::ReplacedByNewRequest);
        }
        self.notices.push(WhiteNotice::FollowChanged(self.follow.record()));
        FollowChange::Changed
    }

    /// Выбор скорости слежения человеком: режим, белую и заявку не трогает.
    pub(super) fn choose_follow_speed(&mut self, speed: WhiteFollowSpeed) -> FollowChange {
        let change = self.follow.choose_speed(speed);
        if change == FollowChange::Changed {
            self.notices.push(WhiteNotice::FollowChanged(self.follow.record()));
        }
        change
    }

    /// Действующая блокировка AE/AF. При слежении она снимает заявку, ответ которой сдвинул бы белую (вопрос
    /// слежения и открытие); уже начатый переезд заканчивает рендер (WB54). Без слежения не делает ничего.
    pub(super) fn note_lock(&mut self, lock: AeAfLock) -> FrameEffect {
        self.follow.note_lock(lock);
        self.cancel_waiting_if_frozen(WhiteCancelCause::AeAfLock)
    }

    /// Пауза слежения от экрана (CALIBRATE, 14n): независима от блокировки AE/AF. Как и блокировка, снимает ждущую
    /// заявку при включённом слежении; начатый переезд доводит рендер. Режим «Auto» и память не трогает.
    pub(super) fn set_follow_pause(&mut self, pause: WhiteFollowPause) -> (FollowChange, FrameEffect) {
        let change = self.follow.set_pause(pause);
        let effect = match change {
            FollowChange::Changed if self.follow.is_held_by_pause() => {
                self.cancel_waiting_if_frozen(WhiteCancelCause::FollowPaused)
            }
            FollowChange::Changed | FollowChange::Unchanged => FrameEffect::Nothing,
        };
        (change, effect)
    }

    /// Слежение заморожено: ждущая заявка (вопрос слежения, открытие, «взять») снимается по причине `by`.
    fn cancel_waiting_if_frozen(&mut self, by: WhiteCancelCause) -> FrameEffect {
        if self.follow.is_frozen() && matches!(self.request, Request::Waiting(_)) {
            self.cancel_request(by);
            return FrameEffect::RequestChanged;
        }
        FrameEffect::Nothing
    }

    /// Выбор правила возобновления человеком. Белую, переход и ожидающую заявку не трогает: правило действует
    /// со следующего возобновления. Выбор, совпавший с умолчанием конфига, всё равно выбор: его запоминаем.
    pub(super) fn choose_resume_rule(&mut self, rule: WhiteResumeRule) -> ResumeRuleChoice {
        if self.chosen_rule == Some(rule) {
            return ResumeRuleChoice::AlreadyChosen;
        }
        self.chosen_rule = Some(rule);
        self.notices.push(WhiteNotice::ResumeRuleChosen(rule));
        ResumeRuleChoice::Chosen
    }

    /// Действующее правило возобновления: выбор человека, а если его нет — умолчание конфига.
    pub(super) fn resume_rule(&self) -> WhiteResumeRule {
        self.chosen_rule.unwrap_or(self.config.resume_rule)
    }

    /// Приложение вернулось на передний план. Единственное место, где действует правило возобновления
    /// (WB22, WB36): по нему и происхождению белой решается, спрашивать ли источник открытия, пока кадры
    /// скрыты. Если не спрашивать — заявка не заводится, кадры не скрываются, белая не меняется ни на бит.
    /// Включённое слежение сильнее правила: спрашивает всегда (WB50), кроме замороженного блокировкой (WB55).
    pub(super) fn on_foreground(&mut self) {
        let source = match self.follow.source() {
            Some(_) if self.follow.is_frozen() => return,
            Some(source) => source,
            None if self.resume_rule().asks_source_on_resume(self.held.origin) => self.opening_source,
            None => return,
        };
        let transition = self.late_transition();
        self.open_request(
            source,
            WhileAwaitingWhite::HideFrames,
            transition,
            None,
            RequestPurpose::Opening,
        );
    }

    /// Приложение ушло в фон: ожидание снимается, белая остаётся.
    pub(super) fn on_background(&mut self) {
        self.cancel_request(WhiteCancelCause::Background);
    }

    /// Пришёл результат повторяющегося запроса: поток жив. Первый из них запускает срок ожидания; слежение по
    /// своему темпу задаёт вопрос, если никто другой источника не спрашивает.
    pub(super) fn on_stream_frame(&mut self, now: Instant, tick: FollowTick) -> FrameEffect {
        self.frame_seen = true;
        self.follow.note_tick(tick);
        let timeout = self.answer_timeout();
        match &mut self.request {
            Request::Waiting(pending) if pending.purpose == RequestPurpose::FollowAsk => {
                let frame = match tick {
                    FollowTick::Frame(frame) => frame,
                    FollowTick::Unstamped => return FrameEffect::Nothing,
                    // Вопрос, заданный до вспышки: ответ на него может прийти уже по её следу.
                    FollowTick::FlashInProgress => {
                        self.request = Request::Idle;
                        return FrameEffect::RequestChanged;
                    }
                };
                if !self.follow.ask_expired(frame, timeout) {
                    return FrameEffect::Nothing;
                }
                // Источник молчит дольше срока: вопрос снимаем, следующий задаст темп.
                self.request = Request::Idle;
                self.follow.note_refused(WhiteRequestOutcome::TimedOut);
                self.ask_for_follow(frame);
                FrameEffect::RequestChanged
            }
            Request::Waiting(pending) => {
                if pending.deadline.is_none() {
                    pending.deadline = Some(now + timeout);
                }
                FrameEffect::Nothing
            }
            Request::Idle => match tick {
                FollowTick::Frame(frame) => self.ask_for_follow(frame),
                FollowTick::Unstamped | FollowTick::FlashInProgress => FrameEffect::Nothing,
            },
        }
    }

    /// Поток кончился (сессия закрыта, камера потеряна, запрос остановлен).
    ///
    /// Заявка, которой поток уже служил, переиздаётся с новым номером: ответ прежнего потока станет чужим, вид
    /// ожидания тот же. Срок открытия снят до первого кадра нового потока; срок команды идёт дальше — иначе
    /// команда без нового потока (камера закрыта, отказ) висела бы вечно и сработала бы при следующем открытии
    /// поверх ручной белой. Если кадров с выдачи заявки не было, ответа
    /// прежнего потока быть не может и заявка остаётся как есть (без лишних номеров на каждой попытке открытия).
    ///
    /// Вопрос слежения не переиздаётся: новый поток получит свой с первого кадра.
    pub(super) fn on_stream_ended(&mut self) {
        let had_frames = std::mem::replace(&mut self.frame_seen, false);
        self.follow.note_stream_ended();
        let Request::Waiting(pending) = self.request else {
            return;
        };
        if pending.purpose == RequestPurpose::FollowAsk {
            self.request = Request::Idle;
            return;
        }
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
        if pending.purpose == RequestPurpose::FollowAsk {
            self.on_follow_answer(pending, answer.outcome);
            return;
        }
        let outcome = match answer.outcome {
            WhiteSourceOutcome::Ready {
                illuminant: SceneIlluminant::Uniform(setting),
                confidence,
            } => {
                self.apply_source_white(&pending, setting);
                WhiteRequestOutcome::Applied { confidence }
            }
            refusal => refusal_outcome(refusal),
        };
        self.finish_request(pending, outcome);
    }

    /// Ответ на вопрос слежения: белая сдвигается, только если ответ вышел из мёртвой зоны. Снаружи видна смена
    /// белой, а не вопрос: ни события, ни строки журнала, ни отчёта у него нет.
    fn on_follow_answer(&mut self, pending: Pending, outcome: WhiteSourceOutcome) {
        self.request = Request::Idle;
        let WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(setting),
            ..
        } = outcome
        else {
            self.follow.note_refused(refusal_outcome(outcome));
            return;
        };
        let candidate = HeldWhite {
            setting: clamp_white(&self.config, setting).setting,
            origin: WhiteOrigin::Source(pending.request.source),
        };
        match self.follow.verdict(self.held, candidate) {
            FollowVerdict::Move => {
                self.follow.note_moved();
                self.apply_source_white(&pending, setting);
            }
            FollowVerdict::InsideDeadZone => self.follow.note_stayed(),
        }
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

    /// Включено ли слежение (в том числе замороженное блокировкой).
    pub(super) fn is_following(&self) -> bool {
        self.follow.source().is_some()
    }

    pub(super) fn held(&self) -> HeldWhite {
        self.held
    }

    /// Заявка, видимая снаружи. Вопросы слежения сюда не попадают: они идут десять раз в секунду, и «ждём ответа»
    /// мигало бы на экране.
    pub(super) fn request_state(&self) -> WhiteRequestState {
        match self.request {
            Request::Idle
            | Request::Waiting(Pending {
                purpose: RequestPurpose::FollowAsk,
                ..
            }) => WhiteRequestState::Idle { last: self.last_result },
            Request::Waiting(pending) => WhiteRequestState::Waiting {
                request: pending.request.id,
                source: pending.request.source,
            },
        }
    }

    /// Снимок для экрана. `saves_requested` ведёт движок (он передаёт записи памяти), поэтому приходит параметром.
    pub(super) fn status(&self, saves_requested: u64) -> WhiteStatus {
        WhiteStatus {
            held: self.held,
            limits: self.limits.clone(),
            range: self.config.range(),
            request: self.request_state(),
            last_take: self.last_take,
            memory: WhiteMemoryState {
                startup: self.memory_startup.clone(),
                saves_requested,
            },
            resume_rule: self.resume_rule(),
            follow: self.follow.status(),
            stale_answers: self.stale_answers,
        }
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
        purpose: RequestPurpose,
    ) {
        self.cancel_request(WhiteCancelCause::ReplacedByNewRequest);
        let id = self.issue_id();
        if let RequestPurpose::Take(command) = purpose {
            self.last_take = Some(WhiteTakeReport::waiting(command));
        }
        self.request = Request::Waiting(Pending {
            request: WhiteSourceRequest {
                id,
                source,
                while_awaiting,
            },
            on_ready,
            deadline,
            purpose,
        });
    }

    fn cancel_request(&mut self, by: WhiteCancelCause) {
        match self.request {
            Request::Waiting(Pending {
                purpose: RequestPurpose::FollowAsk,
                ..
            }) => self.request = Request::Idle,
            Request::Waiting(pending) => self.finish_request(pending, WhiteRequestOutcome::Cancelled { by }),
            Request::Idle => {}
        }
    }

    /// Выключить слежение (действие человека). Его ждущий вопрос снимается молча.
    fn stop_follow(&mut self) -> FollowChange {
        let change = self.follow.stop();
        if change == FollowChange::Changed {
            self.notices.push(WhiteNotice::FollowChanged(self.follow.record()));
        }
        change
    }

    /// Задать вопрос слежения, если темп и блокировка разрешают. Заявки сейчас быть не должно: вызывающий это проверил.
    fn ask_for_follow(&mut self, frame: SensorTimestamp) -> FrameEffect {
        let Some(source) = self.follow.due_source(frame) else {
            return FrameEffect::Nothing;
        };
        self.follow.note_asked(frame);
        let transition = self.follow.speed().transition();
        self.open_request(
            source,
            WhileAwaitingWhite::KeepShowing,
            transition,
            None,
            RequestPurpose::FollowAsk,
        );
        FrameEffect::RequestChanged
    }

    fn finish_request(&mut self, pending: Pending, result: WhiteRequestOutcome) {
        let finished = WhiteRequestResult {
            request: pending.request.id,
            source: pending.request.source,
            result,
        };
        self.request = Request::Idle;
        self.last_result = Some(finished);
        // Отчёт закрывается только если он всё ещё принадлежит команде этой заявки: более новая команда (в том
        // числе отклонённая) свой отчёт уже поставила, и исход старой заявки его не затирает.
        if let (RequestPurpose::Take(command), Some(report)) = (pending.purpose, self.last_take)
            && report.command == command
        {
            self.last_take = Some(report.finished(result));
        }
        self.notices.push(WhiteNotice::Finished(finished));
    }

    /// Принять белую источника по заявке `pending`: прижать к пределам, поставить происхождение «источник».
    fn apply_source_white(&mut self, pending: &Pending, setting: WhiteBalanceSetting) {
        let clamped = clamp_white(&self.config, setting);
        let new_held = HeldWhite {
            setting: clamped.setting,
            origin: WhiteOrigin::Source(pending.request.source),
        };
        self.limits = clamped.adjustments.clone();
        if new_held != self.held {
            self.held = new_held;
            self.transition = pending.on_ready;
            self.notices.push(match pending.purpose {
                RequestPurpose::FollowAsk => WhiteNotice::Followed(new_held),
                RequestPurpose::Opening | RequestPurpose::Take(_) => WhiteNotice::Changed(new_held),
            });
        }
        if !clamped.adjustments.is_empty() {
            self.notices.push(WhiteNotice::Adjusted(clamped.adjustments));
        }
    }
}

/// Отказ источника как итог заявки. Готовый ответ сюда попадает, только если его свет новее владельца: честно «не
/// понял», белая прежняя.
fn refusal_outcome(outcome: WhiteSourceOutcome) -> WhiteRequestOutcome {
    match outcome {
        WhiteSourceOutcome::Ready { .. } => WhiteRequestOutcome::NotUnderstood,
        WhiteSourceOutcome::Busy => WhiteRequestOutcome::Busy,
        WhiteSourceOutcome::Unknown { reason } => WhiteRequestOutcome::Unknown { reason },
        WhiteSourceOutcome::Unavailable { reason } => WhiteRequestOutcome::Unavailable { reason },
        WhiteSourceOutcome::Failed { failure } => WhiteRequestOutcome::Failed { failure },
    }
}
