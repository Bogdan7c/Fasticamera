//! Желаемое состояние и команды: валидация, итог команды и побочные действия.
//!
//! Команда сначала решается чисто ([`CameraEngine::decide_command`]): меняет **только**
//! [`Desired`] (и ничего в железе), возвращает явный итог. Итог публикуется событием
//! `CommandHandled` **до** любых действий с бэкендом, и только потом
//! ([`CameraEngine::apply_command_effects`]) движок доводит реальное состояние до желаемого.
//! Поэтому паника в бэкенде не оставляет команду без ответа.

use std::sync::Arc;
use std::time::Instant;

use camera_contract::capabilities::CameraDescriptor;
use camera_contract::control::CaptureIntent;
use camera_contract::ids::{CameraId, CameraTarget, LensRole};
use camera_contract::stream::StreamMode;
use camera_contract::three_a::AeAfLock;

use super::CameraEngine;
use super::lens_switch::RetargetCause;
use crate::backend::UnsupportedStream;
use crate::command::{CameraCommand, CommandId, CommandOutcome, InvalidStateReason, UnsupportedCommand};
use crate::state::{CameraError, CameraState, ClosedReason, FatalCause};
use crate::status::AppPresence;
use crate::three_a_control::ThreeAIntent;

/// Желаемое состояние: что хочет продукт, независимо от того, получилось ли это пока.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Desired {
    pub(crate) target: Option<CameraTarget>,
    pub(crate) mode: Option<StreamMode>,
    pub(crate) streaming: Option<CaptureIntent>,
    /// Намерение автоматики 3A: точка, блокировка, профиль, ручные параметры.
    pub(crate) three_a: ThreeAIntent,
}

impl CameraEngine {
    /// ID желаемой камеры по каталогу.
    pub(super) fn desired_camera_id(&self) -> Option<CameraId> {
        let target = self.desired.target.as_ref()?;
        self.catalog
            .resolve(target)
            .ok()
            .map(|descriptor| descriptor.id.clone())
    }

    /// Фатальная ошибка, из которой `EnterForeground` даёт свежую попытку.
    fn fatal_is_retryable_on_foreground(&self) -> bool {
        matches!(
            &self.state,
            CameraState::Error {
                error: CameraError::Fatal {
                    cause: FatalCause::RecoveryExhausted { .. } | FatalCause::CatalogUnavailable { .. }
                }
            }
        )
    }

    /// Решить команду: поменять желаемое (если можно) и вернуть итог.
    pub(super) fn decide_command(&mut self, id: CommandId, command: &CameraCommand, now: Instant) -> CommandOutcome {
        if self.poisoned {
            return CommandOutcome::InvalidState(InvalidStateReason::EngineFailed);
        }
        if self.is_fatal() && !self.fatal_allows(command) {
            return CommandOutcome::InvalidState(InvalidStateReason::EngineFailed);
        }
        match command {
            CameraCommand::Open { target } => self.decide_open(target, now),
            CameraCommand::Configure { mode } => self.decide_configure(*mode),
            CameraCommand::StartStreaming { intent } => self.decide_start_streaming(*intent),
            CameraCommand::StopStreaming => self.decide_stop_streaming(),
            CameraCommand::SwitchTo { role } => self.decide_switch_to(*role, RetargetCause::Command, now),
            CameraCommand::ApplyControls { controls } => self.decide_apply_controls(*controls),
            CameraCommand::FocusAndExposeAt { point } => self.decide_point_of_interest(id, *point, AeAfLock::Unlocked),
            CameraCommand::LockFocusAndExposureAt { point } => {
                self.decide_point_of_interest(id, *point, AeAfLock::Locked)
            }
            CameraCommand::ResetPointOfInterest => self.decide_reset_point_of_interest(),
            CameraCommand::SetExposureBias { bias } => self.decide_set_exposure_bias(*bias),
            CameraCommand::SetThreeAProfile { profile } => self.decide_set_three_a_profile(*profile),
            CameraCommand::CaptureBracket { bracket } => self.decide_capture_bracket(id, bracket),
            CameraCommand::Close => self.decide_close(),
            CameraCommand::EnterBackground => self.decide_enter_background(),
            CameraCommand::EnterForeground => self.decide_enter_foreground(),
        }
    }

    /// В `Fatal` исполняются только команды выхода и отпускания.
    fn fatal_allows(&self, command: &CameraCommand) -> bool {
        match command {
            CameraCommand::Close | CameraCommand::EnterBackground => true,
            CameraCommand::EnterForeground => self.fatal_is_retryable_on_foreground(),
            _ => false,
        }
    }

    fn decide_open(&mut self, target: &CameraTarget, now: Instant) -> CommandOutcome {
        let catalog = Arc::clone(&self.catalog);
        let descriptor = match catalog.resolve(target) {
            Ok(descriptor) => descriptor,
            Err(unavailable) => return CommandOutcome::Unsupported(UnsupportedCommand::Target(unavailable)),
        };
        if self.desired_camera_id().as_ref() == Some(&descriptor.id) {
            return CommandOutcome::AlreadyInState;
        }
        if let Some(rejection) = self.desired.mode.and_then(|mode| unsupported_mode(descriptor, &mode)) {
            return CommandOutcome::Unsupported(UnsupportedCommand::Stream(rejection));
        }
        self.desired.target = Some(target.clone());
        self.reset_point_for_camera_change();
        self.note_lens_target_changed(&descriptor.id, RetargetCause::Command, now);
        CommandOutcome::Accepted
    }

    fn decide_configure(&mut self, mode: StreamMode) -> CommandOutcome {
        let Some(target) = self.desired.target.clone() else {
            return CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested);
        };
        let catalog = Arc::clone(&self.catalog);
        let descriptor = match catalog.resolve(&target) {
            Ok(descriptor) => descriptor,
            Err(unavailable) => return CommandOutcome::Unsupported(UnsupportedCommand::Target(unavailable)),
        };
        if let Some(rejection) = unsupported_mode(descriptor, &mode) {
            return CommandOutcome::Unsupported(UnsupportedCommand::Stream(rejection));
        }
        if self.desired.mode == Some(mode) {
            return CommandOutcome::AlreadyInState;
        }
        self.desired.mode = Some(mode);
        // Новый запрос режима проверяется на каждой камере заново.
        self.frame_rate_limits.forget_all();
        CommandOutcome::Accepted
    }

    fn decide_start_streaming(&mut self, intent: CaptureIntent) -> CommandOutcome {
        if self.desired.mode.is_none() {
            return CommandOutcome::InvalidState(InvalidStateReason::NoStreamModeRequested);
        }
        if self.desired.streaming == Some(intent) {
            return CommandOutcome::AlreadyInState;
        }
        self.desired.streaming = Some(intent);
        CommandOutcome::Accepted
    }

    fn decide_stop_streaming(&mut self) -> CommandOutcome {
        if self.desired.streaming.take().is_none() {
            return CommandOutcome::AlreadyInState;
        }
        // Кадров не будет: показанного объектива нет, смена (если шла) не завершится.
        self.reset_lens_switch();
        CommandOutcome::Accepted
    }

    /// Смена цели на камеру роли `role`: общий путь команды `SwitchTo` и намерения экрана
    /// (`cause` решает, снимается ли блокировка роли).
    pub(super) fn decide_switch_to(&mut self, role: LensRole, cause: RetargetCause, now: Instant) -> CommandOutcome {
        if self.desired.target.is_none() {
            return CommandOutcome::InvalidState(InvalidStateReason::NoCameraRequested);
        }
        let catalog = Arc::clone(&self.catalog);
        let target = CameraTarget::Role(role);
        let descriptor = match catalog.resolve(&target) {
            Ok(descriptor) => descriptor,
            Err(unavailable) => return CommandOutcome::Unsupported(UnsupportedCommand::Target(unavailable)),
        };
        if self.desired_camera_id().as_ref() == Some(&descriptor.id) {
            return CommandOutcome::AlreadyInState;
        }
        // Режим и поток переносятся на новую камеру; если формат там не предлагается,
        // желаемое остаётся прежним.
        if let Some(rejection) = self.desired.mode.and_then(|mode| unsupported_mode(descriptor, &mode)) {
            return CommandOutcome::Unsupported(UnsupportedCommand::Stream(rejection));
        }
        self.desired.target = Some(target);
        self.reset_point_for_camera_change();
        self.note_lens_target_changed(&descriptor.id, cause, now);
        CommandOutcome::Accepted
    }

    fn decide_close(&mut self) -> CommandOutcome {
        let nothing_wanted = self.desired == Desired::default();
        let already_closed = matches!(self.state, CameraState::Closed { .. });
        if nothing_wanted && already_closed {
            return CommandOutcome::AlreadyInState;
        }
        self.desired = Desired::default();
        self.frame_rate_limits.forget_all();
        self.reset_lens_switch();
        CommandOutcome::Accepted
    }

    fn decide_enter_background(&mut self) -> CommandOutcome {
        if self.presence == AppPresence::Background {
            return CommandOutcome::AlreadyInState;
        }
        CommandOutcome::Accepted
    }

    /// `EnterForeground` — ещё и сигнал повтора, поэтому «уже на переднем плане» значит
    /// «и повторять нечего».
    fn decide_enter_foreground(&mut self) -> CommandOutcome {
        let has_work = self.presence == AppPresence::Background
            || !self.catalog_loaded
            || self.is_fatal()
            || matches!(self.state, CameraState::Error { .. });
        if has_work {
            CommandOutcome::Accepted
        } else {
            CommandOutcome::AlreadyInState
        }
    }

    /// Действия над реальным состоянием после того, как итог команды уже опубликован.
    pub(super) fn apply_command_effects(&mut self, command: &CameraCommand, outcome: &CommandOutcome, now: Instant) {
        if *outcome != CommandOutcome::Accepted {
            return;
        }
        match command {
            // Новая камера отменяет ожидание восстановления прежней.
            CameraCommand::Open { .. } | CameraCommand::SwitchTo { .. } => {
                self.recovery.clear();
                self.reconcile(now);
            }
            CameraCommand::Configure { .. }
            | CameraCommand::StartStreaming { .. }
            | CameraCommand::StopStreaming
            | CameraCommand::ApplyControls { .. }
            | CameraCommand::FocusAndExposeAt { .. }
            | CameraCommand::LockFocusAndExposureAt { .. }
            | CameraCommand::ResetPointOfInterest
            | CameraCommand::SetExposureBias { .. }
            | CameraCommand::SetThreeAProfile { .. } => self.reconcile(now),
            CameraCommand::CaptureBracket { .. } => self.submit_pending_bracket(now),
            CameraCommand::Close => self.apply_close(),
            CameraCommand::EnterBackground => self.apply_enter_background(),
            CameraCommand::EnterForeground => self.apply_enter_foreground(now),
        }
    }

    fn apply_close(&mut self) {
        self.release_platform();
        self.standby.end_demand();
        self.reconfiguring = false;
        self.recovery.clear();
        if !self.is_fatal() {
            self.transition(CameraState::Closed {
                reason: ClosedReason::NotRequested,
            });
        }
    }

    /// В фоне камера отпущена, попыток открытия нет; желаемое сохранено.
    fn apply_enter_background(&mut self) {
        self.presence = AppPresence::Background;
        self.release_platform();
        self.standby.end_demand();
        self.reconfiguring = false;
        self.recovery.stop_waiting();
        if !self.is_fatal() {
            self.transition(CameraState::Closed {
                reason: ClosedReason::AppInBackground,
            });
        }
    }

    /// Возврат на передний план: счётчик попыток с нуля и довести до желаемого.
    fn apply_enter_foreground(&mut self, now: Instant) {
        self.presence = AppPresence::Foreground;
        self.recovery.reset_attempts();
        if self.is_fatal() {
            self.transition(CameraState::Closed {
                reason: ClosedReason::NotRequested,
            });
        }
        if !self.catalog_loaded {
            self.load_catalog(now);
            if self.is_fatal() {
                return;
            }
        }
        if self.desired.target.is_some() && self.recovery.unresolved.is_some() {
            self.recovery.start_attempt(&mut self.counters);
        }
        self.recovery.stop_waiting();
        self.reconcile(now);
    }
}

/// Режим, которого камера не предлагает (формат RAW не в карте потоков), либо `None`.
fn unsupported_mode(descriptor: &CameraDescriptor, mode: &StreamMode) -> Option<UnsupportedStream> {
    descriptor
        .capabilities
        .largest_raw_stream(mode.raw_format)
        .is_none()
        .then_some(UnsupportedStream::FormatNotOffered {
            format: mode.raw_format,
        })
}
