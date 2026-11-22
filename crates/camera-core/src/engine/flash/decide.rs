//! Решение по команде `CaptureFlashStill` и итог отказа.
//!
//! Порядок: сначала `decide_capture_flash_still` (ничего не меняет), потом, уже ПОСЛЕ события
//! `CommandHandled`, `settle_flash_command`: принятая команда запускает предвспышку, отказанная
//! даёт итог `Failed` с причиной. Так итог снимка всегда идёт после ответа на команду.

use std::time::Instant;

use camera_contract::capabilities::RequestCapability;
use camera_contract::flash::FlashStillFailure;

use super::super::CameraEngine;
use crate::command::{CameraCommand, CommandOutcome, InvalidStateReason, UnsupportedCommand, UnsupportedThreeA};
use crate::flash_status::FlashStillReport;
use crate::three_a_control::SceneForFlash;

impl CameraEngine {
    /// Можно ли снять со вспышкой прямо сейчас. Состояние не меняет.
    pub(in crate::engine) fn decide_capture_flash_still(&self) -> CommandOutcome {
        if !self.is_streaming_now() {
            return CommandOutcome::InvalidState(InvalidStateReason::NotStreaming);
        }
        if self.flash.in_progress() {
            return CommandOutcome::InvalidState(InvalidStateReason::FlashInProgress);
        }
        // Серия и вспышка делят повторяющийся запрос и сроки: одновременно нельзя.
        if self.three_a.bracket_in_progress() {
            return CommandOutcome::InvalidState(InvalidStateReason::BracketInProgress);
        }
        let capabilities = self.planning_capabilities();
        if !capabilities.as_ref().is_some_and(|caps| caps.flash.is_present()) {
            return CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::FlashUnit));
        }
        let has_manual_sensor = capabilities
            .as_ref()
            .is_some_and(|caps| caps.has_capability(RequestCapability::ManualSensor));
        if !has_manual_sensor {
            return CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor));
        }
        // Вспышку считает только наш AE: вендорский не отдаёт ни света сцены, ни замеров предвспышки.
        if self.three_a.scene_for_flash() == SceneForFlash::NotOwnExposure {
            return CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::FlashNeedsOwnExposure));
        }
        CommandOutcome::Accepted
    }

    /// Последствия команды вспышки после того, как её итог уже в событиях. Остальные команды не трогает.
    pub(in crate::engine) fn settle_flash_command(
        &mut self,
        command: &CameraCommand,
        outcome: &CommandOutcome,
        now: Instant,
    ) {
        let CameraCommand::CaptureFlashStill { shot } = command else {
            return;
        };
        self.flash.counters.requested += 1;
        if *outcome == CommandOutcome::Accepted {
            self.begin_flash_preflash(*shot, now);
        } else {
            // Отказ — тоже итог снимка: рендер ждёт ровно один итог на команду.
            self.flash.counters.failed += 1;
            let failure = failure_for_refusal(outcome);
            self.emit_flash_report(FlashStillReport::Failed { shot: *shot, failure });
        }
    }
}

/// Причина итога снимка по отказу команды. Одна чистая функция на все отказы.
///
/// `EngineFailed` (движок упал или в `Fatal`) отображается в `SessionLost`: камера и её поток
/// потеряны, а отдельной причины «движок мёртв» итог снимка не знает. Остальные исходы
/// `decide_capture_flash_still` не выдаёт; если когда-нибудь выдаст, честнее всего сказать
/// «поток не идёт».
pub(super) fn failure_for_refusal(outcome: &CommandOutcome) -> FlashStillFailure {
    match outcome {
        CommandOutcome::InvalidState(InvalidStateReason::NotStreaming) => FlashStillFailure::NotStreaming,
        CommandOutcome::InvalidState(InvalidStateReason::FlashInProgress | InvalidStateReason::BracketInProgress) => {
            FlashStillFailure::AlreadyInProgress
        }
        CommandOutcome::InvalidState(InvalidStateReason::EngineFailed) => FlashStillFailure::SessionLost,
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::FlashUnit)) => {
            FlashStillFailure::NoFlashUnit
        }
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor)) => {
            FlashStillFailure::NeedsManualSensor
        }
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::FlashNeedsOwnExposure)) => {
            FlashStillFailure::NeedsOwnExposure
        }
        _ => FlashStillFailure::NotStreaming,
    }
}
