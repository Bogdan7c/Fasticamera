//! Точки входа `CameraEngine` в владельца белого: команды, возобновление, поток, ответы источника, сроки.
//!
//! Правило событий: итог команды (`CommandHandled`) идёт раньше событий белой, которые она вызвала, поэтому
//! решение команды только меняет владельца, а события выкладываются отдельным вызовом
//! [`CameraEngine::flush_white_events`] — после итога.

use std::time::Instant;

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::{WhiteAdjustment, WhiteSourceId, WhiteTransition};

use super::owner::ManualOutcome;
use crate::command::{CameraCommand, CommandId, CommandOutcome, InvalidStateReason};
use crate::engine::CameraEngine;

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

    /// Приложение вернулось с заднего плана.
    pub(in crate::engine) fn white_entered_foreground(&mut self) {
        self.white.on_foreground();
    }

    /// Приложение ушло в фон.
    pub(in crate::engine) fn white_entered_background(&mut self) {
        self.white.on_background();
    }

    /// Пришёл результат повторяющегося запроса.
    pub(in crate::engine) fn note_white_stream_frame(&mut self, now: Instant) {
        self.white.on_stream_frame(now);
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
