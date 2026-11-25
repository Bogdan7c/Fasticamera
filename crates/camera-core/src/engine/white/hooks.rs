//! Точки входа `CameraEngine` в владельца белого: команды, возобновление, поток, ответы источника, сроки.
//!
//! Правило событий: итог команды (`CommandHandled`) идёт раньше событий белой, которые она вызвала, поэтому
//! решение команды только меняет владельца, а события выкладываются отдельным вызовом
//! [`CameraEngine::flush_white_events`] — после итога.

use std::time::Instant;

use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::{WhiteAdjustment, WhiteSourceId, WhiteTransition};

use super::follow::{FollowChange, FollowTick};
use super::owner::ManualOutcome;
use crate::command::{CameraCommand, CommandId, CommandOutcome, InvalidStateReason};
use crate::engine::CameraEngine;
use crate::white::{ResumeRuleChoice, WhiteFollowSpeed, WhiteResumeRule};

impl CameraEngine {
    /// Команда `SetWhite`: белая рукой. Работает и без потока: белая держится и ждёт камеру.
    pub(in crate::engine) fn decide_set_white(
        &mut self,
        setting: WhiteBalanceSetting,
        transition: WhiteTransition,
    ) -> CommandOutcome {
        match self.white.set_manual(setting, transition) {
            ManualOutcome::Changed => CommandOutcome::Accepted,
            ManualOutcome::AlreadyHeld => CommandOutcome::AlreadyInState,
        }
    }

    /// Команда `AdjustWhite`: правка одной оси рукой. Вторая ось берётся у владельца в этот момент. Работает и
    /// без потока.
    pub(in crate::engine) fn decide_adjust_white(
        &mut self,
        adjustment: WhiteAdjustment,
        transition: WhiteTransition,
    ) -> CommandOutcome {
        match self.white.adjust_manual(adjustment, transition) {
            ManualOutcome::Changed => CommandOutcome::Accepted,
            ManualOutcome::AlreadyHeld => CommandOutcome::AlreadyInState,
        }
    }

    /// Команда `SetWhiteResumeRule`: выбор правила возобновления. Работает и без потока, и в фоне; белую,
    /// переход и ожидающую заявку не трогает.
    pub(in crate::engine) fn decide_set_white_resume_rule(&mut self, rule: WhiteResumeRule) -> CommandOutcome {
        match self.white.choose_resume_rule(rule) {
            ResumeRuleChoice::Chosen => CommandOutcome::Accepted,
            ResumeRuleChoice::AlreadyChosen => CommandOutcome::AlreadyInState,
        }
    }

    /// Команда `FollowWhite`: следить за источником. Работает и без потока, и в фоне: спросит первый кадр.
    pub(in crate::engine) fn decide_follow_white(&mut self, source: WhiteSourceId) -> CommandOutcome {
        outcome_of(self.white.follow_source(source))
    }

    /// Команда `SetWhiteFollowSpeed`: скорость слежения. Режим не включает и белую не трогает.
    pub(in crate::engine) fn decide_set_white_follow_speed(&mut self, speed: WhiteFollowSpeed) -> CommandOutcome {
        outcome_of(self.white.choose_follow_speed(speed))
    }

    /// Команда `TakeWhite`: взять белую у источника. Нужен идущий поток — иначе спрашивать нечем.
    /// Знает ли рендер такой источник, движок не проверяет: рендер ответит `Unavailable(NotRegistered)`.
    pub(in crate::engine) fn decide_take_white(
        &mut self,
        command: CommandId,
        source: WhiteSourceId,
        transition: WhiteTransition,
        now: Instant,
    ) -> CommandOutcome {
        if !self.is_streaming_now() {
            return CommandOutcome::InvalidState(InvalidStateReason::NotStreaming);
        }
        self.white.take_from_source(command, source, transition, now);
        CommandOutcome::Accepted
    }

    /// Итог команды решён: если это отклонённая `TakeWhite`, экран получает отчёт «не спрашивали» с причиной.
    /// Один вход на все отказы (нет потока, `Fatal`, сломанный движок): отдельные ветки решения такой отказ
    /// не покрыли бы целиком, потому что `Fatal` отсекает команду раньше.
    pub(in crate::engine) fn note_white_take_outcome(
        &mut self,
        id: CommandId,
        command: &CameraCommand,
        outcome: &CommandOutcome,
    ) {
        if let (CameraCommand::TakeWhite { .. }, CommandOutcome::InvalidState(reason)) = (command, outcome) {
            self.white.note_take_not_asked(id, *reason);
        }
    }

    /// Передать владельцу белого действующую блокировку AE/AF (WB46). Блокировку меняют несколько мест движка
    /// (тап, сброс точки, смена камеры, закрытие), поэтому владелец не подписывается на каждое, а получает текущее
    /// значение: сразу после решения команды (ставят блокировку только команды, и ответ, идущий навстречу, уже
    /// чужой) и на кадре — снять её может и смена линзы по намерению экрана, минуя команды.
    pub(in crate::engine) fn sync_white_lock(&mut self) {
        self.white.note_lock(self.desired.three_a.lock);
    }

    /// Приложение вернулось с заднего плана.
    pub(in crate::engine) fn white_entered_foreground(&mut self) {
        self.white.on_foreground();
    }

    /// Приложение ушло в фон.
    pub(in crate::engine) fn white_entered_background(&mut self) {
        self.white.on_background();
    }

    /// Пришёл результат повторяющегося запроса. Про снимок со вспышкой слежение узнаёт отсюда: пока он идёт (и немного
    /// после) оно источника не спрашивает.
    pub(in crate::engine) fn note_white_stream_frame(&mut self, result: &CaptureResultMeta, now: Instant) {
        self.sync_white_lock();
        let tick = if self.flash.in_progress() {
            FollowTick::FlashInProgress
        } else {
            result.sensor_timestamp.map_or(FollowTick::Unstamped, FollowTick::Frame)
        };
        self.white.on_stream_frame(now, tick);
    }

    /// Поток закончился.
    pub(in crate::engine) fn end_white_stream(&mut self) {
        self.white.on_stream_ended();
    }

    /// Толчок «в ящике что-то новое»: прочитать ответ источника.
    pub(in crate::engine) fn poll_white_answer(&mut self) {
        self.white.poll_answer();
    }

    /// Сработал таймер: срок ожидания ответа мог выйти.
    pub(in crate::engine) fn fire_white_deadline(&mut self, now: Instant) {
        self.white.on_deadline(now);
    }

    /// Выложить накопленные события белой в общий поток событий движка.
    pub(in crate::engine) fn flush_white_events(&mut self) {
        let events = self.white.take_events();
        self.events.extend(events);
    }
}

/// Итог команды слежения.
fn outcome_of(change: FollowChange) -> CommandOutcome {
    match change {
        FollowChange::Changed => CommandOutcome::Accepted,
        FollowChange::Unchanged => CommandOutcome::AlreadyInState,
    }
}
