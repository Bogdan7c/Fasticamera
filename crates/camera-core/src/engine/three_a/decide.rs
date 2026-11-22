//! Итоги команд 3A: тап, блокировка, сброс точки, компенсация, профиль, ручные параметры, серия.
//!
//! Правила:
//! * все они меняют только `Desired::three_a` (кроме серии — она разовая и живёт в трекере серии, см. `bracket`);
//! * тап, блокировка и серия нужны идущему потоку: иначе `InvalidState(NotStreaming)`;
//! * тап на камере с фиксированным фокусом **принимается**: меняется точка замера
//!   экспозиции, а фокус честно остаётся `FixedLens`;
//! * кандидат-намерение проверяется исполнителями (`validate_three_a_intent`) до принятия:
//!   отказ — `Unsupported`, желаемое не меняется, обращений к бэкенду нет;
//! * то же значение ещё раз — `AlreadyInState`.

use camera_contract::capabilities::RequestCapability;
use camera_contract::control::{ExposureBias, ManualControl};
use camera_contract::three_a::{
    AeAfLock, ExposureBracket, MeteringRegion, PointOfInterest, PointRequestId, SensorPoint, ThreeAProfile,
};

use super::super::{CameraEngine, LOG_TARGET};
use super::bracket::BracketRun;
use crate::command::{CommandId, CommandOutcome, InvalidStateReason, UnsupportedCommand, UnsupportedThreeA};
use crate::three_a_control::ThreeAIntent;

impl CameraEngine {
    /// Повторяющийся запрос реально идёт (а не только «хотим поток»).
    pub(in crate::engine) fn is_streaming_now(&self) -> bool {
        self.session.as_ref().is_some_and(|session| session.repeating.is_some())
    }

    /// Регион замера вокруг точки по размеру из конфига; `None` — конфиг не прошёл проверку.
    fn metering_region_at(&self, point: SensorPoint) -> Option<MeteringRegion> {
        MeteringRegion::new(point, self.config.three_a.metering_region_size_fraction)
            .map_err(|error| tracing::error!(target: LOG_TARGET, %error, "размер региона замера из конфига неверен"))
            .ok()
    }

    /// Принять кандидата-намерение, если исполнители согласны.
    fn accept_three_a_intent(&mut self, candidate: ThreeAIntent) -> CommandOutcome {
        if candidate == self.desired.three_a {
            return CommandOutcome::AlreadyInState;
        }
        match self.validate_three_a_intent(&candidate) {
            Ok(()) => {
                self.desired.three_a = candidate;
                CommandOutcome::Accepted
            }
            Err(unsupported) => CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(unsupported)),
        }
    }

    /// Тап (`lock = Unlocked`) или долгий тап (`lock = Locked`) в точке. Номер тапа — номер команды.
    pub(in crate::engine) fn decide_point_of_interest(
        &mut self,
        id: CommandId,
        point: SensorPoint,
        lock: AeAfLock,
    ) -> CommandOutcome {
        if !self.is_streaming_now() {
            return CommandOutcome::InvalidState(InvalidStateReason::NotStreaming);
        }
        let Some(region) = self.metering_region_at(point) else {
            return CommandOutcome::InvalidState(InvalidStateReason::EngineFailed);
        };
        let mut candidate = self.desired.three_a;
        candidate.point = PointOfInterest::Point {
            region,
            request: PointRequestId(id.0),
        };
        candidate.lock = lock;
        self.accept_three_a_intent(candidate)
    }

    pub(in crate::engine) fn decide_reset_point_of_interest(&mut self) -> CommandOutcome {
        let mut candidate = self.desired.three_a;
        candidate.point = PointOfInterest::WholeFrame;
        candidate.lock = AeAfLock::Unlocked;
        self.accept_three_a_intent(candidate)
    }

    pub(in crate::engine) fn decide_set_exposure_bias(&mut self, bias: ExposureBias) -> CommandOutcome {
        let mut candidate = self.desired.three_a;
        candidate.controls.exposure_bias = bias;
        self.accept_three_a_intent(candidate)
    }

    pub(in crate::engine) fn decide_set_three_a_profile(&mut self, profile: ThreeAProfile) -> CommandOutcome {
        let mut candidate = self.desired.three_a;
        candidate.profile = profile;
        self.accept_three_a_intent(candidate)
    }

    pub(in crate::engine) fn decide_apply_controls(&mut self, controls: ManualControl) -> CommandOutcome {
        let mut candidate = self.desired.three_a;
        candidate.controls = controls;
        self.accept_three_a_intent(candidate)
    }

    /// Серия с брекетингом: нужен идущий поток, ручной сенсор, длина в пределах конфига и
    /// отсутствие незавершённой серии. Принятая серия ждёт отправки в `apply_command_effects`.
    pub(in crate::engine) fn decide_capture_bracket(
        &mut self,
        id: CommandId,
        bracket: &ExposureBracket,
    ) -> CommandOutcome {
        if !self.is_streaming_now() {
            return CommandOutcome::InvalidState(InvalidStateReason::NotStreaming);
        }
        // Серия и вспышка делят повторяющийся запрос и сроки: одновременно нельзя.
        if self.flash.in_progress() {
            return CommandOutcome::InvalidState(InvalidStateReason::FlashInProgress);
        }
        if self.three_a.bracket.is_some() {
            return CommandOutcome::InvalidState(InvalidStateReason::BracketInProgress);
        }
        let has_manual_sensor = self
            .planning_capabilities()
            .is_some_and(|capabilities| capabilities.has_capability(RequestCapability::ManualSensor));
        if !has_manual_sensor {
            return CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor));
        }
        // Команда несёт уже собранную серию, а предел мог быть другим при сборке: проверяем по конфигу движка.
        if bracket.frames().len() > self.config.three_a.max_bracket_frames {
            return CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::BracketTooLong));
        }
        self.three_a.bracket = Some(BracketRun::accepted(
            camera_contract::three_a::BurstId(id.0),
            bracket.clone(),
        ));
        CommandOutcome::Accepted
    }

    /// Смена камеры: точка и блокировка относились к прошлой камере (оценка по iOS, конфиг).
    pub(in crate::engine) fn reset_point_for_camera_change(&mut self) {
        if self.config.three_a.reset_point_on_camera_switch {
            self.desired.three_a.point = PointOfInterest::WholeFrame;
            self.desired.three_a.lock = AeAfLock::Unlocked;
        }
    }
}
