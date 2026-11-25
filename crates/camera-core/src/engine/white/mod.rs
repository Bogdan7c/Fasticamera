//! Белая в движке: владелец ([`owner::WhiteOwner`]), его границы с рендером и памятью, и точки входа движка.
//!
//! Раскладка:
//! * `owner` — чистая машина состояний (кто держит белую, какая заявка источнику, сроки);
//! * `journal` — строки журнала о белой (редкие события, не на кадр);
//! * `limits` — прижим значения к пределам конфига;
//! * этот файл — [`WhiteEngine`]: машина + ящик указаний рендеру + ящик ответов + память; он превращает
//!   уведомления машины в события и записи памяти и публикует указание, **когда оно изменилось**;
//! * `hooks` — методы `CameraEngine`: команды, возобновление, поток, ответы, сроки.
//!
//! Белая живёт **вне** `Desired`: `Close`, `ApplyControls`, `SwitchTo` её не трогают (WB25, WB27).

mod hooks;
mod journal;
mod limits;
mod owner;
#[cfg(test)]
mod owner_tests;

use std::time::Instant;

use camera_contract::three_a::{DirectiveGeneration, DirectiveSlot};
use camera_contract::white::{
    WhiteAdjustment, WhiteAnswerChannel, WhiteDirective, WhiteOrigin, WhiteSourceId, WhiteTransition,
};

use crate::command::{CommandId, InvalidStateReason};
use crate::event::CameraEvent;
use crate::white::{
    RememberedOrigin, WhiteConfig, WhiteMemory, WhiteMemoryRecord, WhiteMemoryStartup, WhiteStatus, WhiteWiring,
};
use owner::{ManualOutcome, WhiteNotice, WhiteOwner};

/// Владелец белого вместе с его границами.
pub(super) struct WhiteEngine {
    owner: WhiteOwner,
    directives: DirectiveSlot<WhiteDirective>,
    /// Что опубликовано последним: то же самое второй раз не публикуется.
    last_published: Option<WhiteDirective>,
    answers: WhiteAnswerChannel,
    /// Поколение последнего прочитанного ответа.
    seen_answer: Option<DirectiveGeneration>,
    memory: Box<dyn WhiteMemory>,
    saves_requested: u64,
    /// Память приняла записи, которые с прошлого `flush` не сбрасывались: уход в фон должен её подтолкнуть.
    unflushed_saves: bool,
    /// События, ещё не отданные движку (движок выкладывает их в нужный момент относительно итога команды).
    events: Vec<CameraEvent>,
}

impl WhiteEngine {
    /// Владелец со стартовой белой; указание публикуется сразу, чтобы рендер видел белую с первого кадра.
    /// Заявка источнику при создании не заводится: «возобновление» (WB22) — это возврат из фона, а приложение
    /// стартует в фоне и получает `EnterForeground` на первом `Resumed`.
    pub(super) fn new(config: WhiteConfig, wiring: WhiteWiring) -> Self {
        // Номера заявок не переиспользуются и между запусками движка над одним ящиком ответов:
        // ответ, оставшийся от прежнего движка, не должен совпасть с первой заявкой нового.
        let first_request = wiring
            .answers
            .newer_than(None)
            .map_or(1, |(_, answer)| answer.request.0.saturating_add(1));
        let mut engine = Self {
            owner: WhiteOwner::new(config, wiring.opening_source, wiring.remembered, first_request),
            directives: wiring.directives,
            last_published: None,
            answers: wiring.answers,
            seen_answer: None,
            memory: wiring.memory,
            saves_requested: 0,
            unflushed_saves: false,
            events: Vec::new(),
        };
        if let WhiteMemoryStartup::Unreadable { reason } = engine.owner.memory_startup() {
            tracing::warn!(target: journal::LOG_TARGET, %reason, "память белой не читается, взят запасной свет");
        }
        journal::white_started(&engine.owner.held());
        engine.settle();
        engine
    }

    /// Рука.
    pub(super) fn set_manual(
        &mut self,
        setting: camera_contract::control::WhiteBalanceSetting,
        transition: WhiteTransition,
    ) -> ManualOutcome {
        let outcome = self.owner.set_manual(setting, transition);
        self.settle();
        outcome
    }

    /// Рука, одна ось (вторая — у владельца).
    pub(super) fn adjust_manual(&mut self, adjustment: WhiteAdjustment, transition: WhiteTransition) -> ManualOutcome {
        let outcome = self.owner.adjust_manual(adjustment, transition);
        self.settle();
        outcome
    }

    pub(super) fn take_from_source(
        &mut self,
        command: CommandId,
        source: WhiteSourceId,
        transition: WhiteTransition,
        now: Instant,
    ) {
        self.owner.take_from_source(command, source, transition, now);
        self.settle();
    }

    /// Команда «взять белую» отклонена до заявки.
    pub(super) fn note_take_not_asked(&mut self, command: CommandId, reason: InvalidStateReason) {
        self.owner.note_take_not_asked(command, reason);
    }

    pub(super) fn on_foreground(&mut self) {
        self.owner.on_foreground();
        self.settle();
    }

    /// Уход в фон: заявка снимается, а то, что память приняла, но могла отложить (правка шкалы под пальцем),
    /// ложится на диск сразу: фоновый процесс могут убить без предупреждения. `flush` диск на потоке движка не
    /// ждёт. Звать его без новых записей незачем, поэтому движок помнит, было ли несброшенное.
    pub(super) fn on_background(&mut self) {
        self.owner.on_background();
        self.settle();
        if std::mem::take(&mut self.unflushed_saves) {
            self.memory.flush();
        }
    }

    pub(super) fn on_stream_frame(&mut self, now: Instant) {
        // Кадров много, а срок меняется только на первом: не трогаем указание и события на каждом.
        self.owner.on_stream_frame(now);
    }

    pub(super) fn on_stream_ended(&mut self) {
        self.owner.on_stream_ended();
        self.settle();
    }

    /// Прочитать ответ источника, если он новее прочитанного.
    pub(super) fn poll_answer(&mut self) {
        if let Some((generation, answer)) = self.answers.newer_than(self.seen_answer) {
            self.seen_answer = Some(generation);
            self.owner.on_answer(&answer);
            self.settle();
        }
    }

    pub(super) fn on_deadline(&mut self, now: Instant) {
        self.owner.on_deadline(now);
        self.settle();
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.owner.next_deadline()
    }

    /// Белая и заявка сейчас (для статуса и тестов).
    pub(super) fn status(&self) -> WhiteStatus {
        self.owner.status(self.saves_requested)
    }

    /// События, накопленные с прошлого вызова.
    pub(super) fn take_events(&mut self) -> Vec<CameraEvent> {
        std::mem::take(&mut self.events)
    }

    /// После каждого входа: уведомления машины → события и память, указание → рендеру (если изменилось).
    fn settle(&mut self) {
        for notice in self.owner.drain_notices() {
            match notice {
                WhiteNotice::Changed(held) => {
                    journal::white_changed(&held);
                    self.remember(held);
                    self.events.push(CameraEvent::WhiteChanged { held });
                }
                WhiteNotice::Adjusted(adjustments) => {
                    self.events.push(CameraEvent::ControlsAdjusted { adjustments });
                }
                WhiteNotice::Finished(result) => {
                    journal::request_finished(&result);
                    self.events.push(CameraEvent::WhiteRequestFinished { result });
                }
            }
        }
        let directive = self.owner.directive();
        if self.last_published != Some(directive) {
            self.directives.publish(directive);
            self.last_published = Some(directive);
        }
    }

    /// Передать памяти белую, которую стоит помнить; запасной свет не сохраняется.
    fn remember(&mut self, held: camera_contract::white::HeldWhite) {
        let origin = match held.origin {
            WhiteOrigin::Source(_) => RememberedOrigin::Source,
            WhiteOrigin::Manual => RememberedOrigin::Manual,
            WhiteOrigin::Fallback => return,
        };
        self.saves_requested += 1;
        self.unflushed_saves = true;
        self.memory.save(WhiteMemoryRecord {
            setting: held.setting,
            origin,
        });
    }
}

/// Снимок белого, каким его видит экран ДО первой публикации движка: запомненная белая (а не запасной свет) и
/// пределы из конфига. Строится тем же владельцем, что потом поведёт движок, поэтому значения совпадают.
pub(crate) fn initial_white_status(config: &WhiteConfig, wiring: &WhiteWiring) -> WhiteStatus {
    // Номер первой заявки в снимок не попадает, а движок выберет свой по ящику ответов.
    WhiteOwner::new(*config, wiring.opening_source, wiring.remembered.clone(), 1).status(0)
}
