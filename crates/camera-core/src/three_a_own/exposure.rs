//! Исполнитель экспозиции «наш AE по статистике RAW» (сессия 08a, решения 63, 66, 68).
//!
//! Роль: рендер меряет сцену и предлагает «сколько света нужно» ([`ExposureProposal`]); этот
//! исполнитель — «тренер» на стороне движка: принимает предложения, отсеивает устаревшие,
//! следит за мерцанием и строит `ExposurePlan::Sensor` (`AE_MODE_OFF`).
//!
//! # Кто что меняет
//! * [`ExposureController::plan`] — **чистая функция** принятого состояния и контекста. Она
//!   зовётся и в `validate` для кандидатов, поэтому ничего не меняет (проверено тестом).
//! * Принятое состояние меняют только `refresh()` (новое предложение, смена мерцания),
//!   `observe_result()` (детектор мерцания, метка начала потока) и `reset_for_new_stream()`.
//! * План читает «основу» ([`PlanBasis`]-поля `driving_target` и `planned_flicker`), которая
//!   обновляется **только** в `refresh()` и только когда он отвечает `Changed`. Мелкий дрейф
//!   предложений ниже `replan_min_change_ev` запрос не перестраивает и план не двигает.
//!
//! # Память прошлого потока и смена камеры
//! Пока нет предложения нового потока, план стартует с цели из памяти прошлого потока
//! (последнее принятое предложение), иначе из конфига. Память записана в произведении прошлой
//! камеры, поэтому при смене камеры движок, который знает обе, пересчитывает её через
//! `exposure_transfer` ([`ExposureController::carry_memory_to_new_camera`], решения 118 и 126).
//!
//! # Блокировка
//! Исполнитель намерения в `refresh()` не знает. Блокировку ведёт рендер: после
//! `ProposalConvergence::Holding` цель заморожена, и новые предложения с той же целью
//! план не перестраивают (порог `replan_min_change_ev`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use camera_contract::auto_exposure::{
    CameraTag, ExposureProduct, ExposureProposal, ExposureProposalChannel, ProposalConvergence,
};
use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::ids::CameraId;
use camera_contract::three_a::{DirectiveGeneration, MainsFrequency, RequestOrigin};

use super::config::OwnExposureConfig;
use super::flicker::FlickerProtection;
use super::plan::{PlanBasis, plan_exposure};
use super::proposal_gate::{GateVerdict, ProposalGate};
use crate::command::UnsupportedThreeA;
use crate::exposure_transfer::ExposureTransfer;
use crate::three_a_control::{ExposureController, ExposureDecision, PlanRefresh, ThreeAContext};
use crate::three_a_status::AutoExposureProgress;

/// Наш AE на стороне движка.
pub struct RawStatsExposure {
    config: OwnExposureConfig,
    proposals: ExposureProposalChannel,
    /// Поколение последнего прочитанного из ящика предложения. Переживает смену потока:
    /// ящик хранит последнее значение, и его нельзя читать второй раз.
    seen: Option<DirectiveGeneration>,
    gate: ProposalGate,
    /// Последнее принятое предложение (статус и память); план его не читает.
    accepted: Option<ExposureProposal>,
    /// Цель, по которой построен текущий план; `None` — стартовое значение.
    driving_target: Option<ExposureProduct>,
    /// Защита от мерцания: задана человеком (конфиг + частота сети), за поток не меняется.
    flicker: FlickerProtection,
    /// Цель последнего принятого предложения прошлого потока.
    memory: Option<ExposureProduct>,
    /// Стартовая цель из конфига; `None` только при заведомо негодном конфиге (недостижимо
    /// после `validate()`): тогда без предложения и памяти план честно отказывает.
    configured_start: Option<ExposureProduct>,
}

impl RawStatsExposure {
    pub fn new(config: OwnExposureConfig, proposals: ExposureProposalChannel, mains: MainsFrequency) -> Self {
        let flicker = FlickerProtection::new(config.flicker, mains);
        let configured_start = ExposureProduct::from_seconds_iso(config.initial_exposure_seconds_iso).ok();
        Self {
            config,
            proposals,
            seen: None,
            gate: ProposalGate::default(),
            flicker,
            accepted: None,
            driving_target: None,
            memory: None,
            configured_start,
        }
    }

    /// Сколько предложений отброшено как относящиеся к прошлому потоку. Счётчик общий:
    /// им можно пользоваться и после того, как исполнитель отдан движку.
    pub fn stale_proposals_counter(&self) -> Arc<AtomicU64> {
        self.gate.stale_counter()
    }

    /// Сколько предложений отброшено как измеренных на другой камере, чем текущий поток (13b). Счётчик общий.
    pub fn foreign_camera_proposals_counter(&self) -> Arc<AtomicU64> {
        self.gate.foreign_camera_counter()
    }

    /// Цель до первого предложения: память прошлого потока, иначе конфиг.
    fn start_target(&self) -> Option<ExposureProduct> {
        self.memory.or(self.configured_start)
    }

    /// Цель, на которой стоит текущий план.
    fn current_target(&self) -> Option<ExposureProduct> {
        self.driving_target.or_else(|| self.start_target())
    }

    fn basis(&self) -> Option<PlanBasis> {
        Some(PlanBasis {
            target: self.current_target()?,
            flicker: self.flicker,
            not_reached_tolerance_ev: self.config.not_reached_tolerance_ev,
            shortfall_report_step_ev: self.config.shortfall_report_step_ev,
        })
    }

    /// Принять цель предложения; `Changed`, если она отличается от текущей основы плана
    /// не меньше порога.
    fn adopt(&mut self, proposal: ExposureProposal) -> PlanRefresh {
        self.accepted = Some(proposal);
        let below_threshold = self.current_target().is_some_and(|current| {
            proposal.target.ev_above(current).abs() < f64::from(self.config.replan_min_change_ev)
        });
        if below_threshold {
            return PlanRefresh::Unchanged;
        }
        self.driving_target = Some(proposal.target);
        PlanRefresh::Changed
    }

    /// Толкнуть движок: «загляни в `refresh()`». Нужен, когда решение выросло не из ящика
    /// рендера (придержанное предложение дождалось метки потока).
    fn nudge_engine(&self) {
        // Если движок ещё не привязан, толчок пропущен, но ящик сигнала это помнит: `bind`
        // дошлёт получателю один толчок, и движок вызовет `refresh()`.
        let _delivery = self.proposals.refresh_signal().signal();
    }
}

impl ExposureController for RawStatsExposure {
    fn plan(&mut self, context: &ThreeAContext<'_>) -> Result<ExposureDecision, UnsupportedThreeA> {
        // Негодный конфиг без памяти и предложений: честный отказ, движок деградирует сам.
        let basis = self.basis().ok_or(UnsupportedThreeA::ManualSensor)?;
        plan_exposure(&basis, context)
    }

    fn observe_result(&mut self, result: &CaptureResultMeta) {
        // Разовые кадры (триггер фокуса, серия) идут с чужими параметрами экспозиции;
        // метку начала потока ставит только повторяющийся запрос. Мерцание из результата
        // (`scene_flicker`) сюда не читается: детектор HAL фиктивный, решение 77.
        if result.three_a.origin != RequestOrigin::Repeating {
            return;
        }
        if let Some(timestamp) = result.sensor_timestamp
            && self.gate.mark_stream_start(timestamp)
        {
            self.nudge_engine();
        }
    }

    fn refresh(&mut self) -> PlanRefresh {
        if let Some((generation, proposal)) = self.proposals.newer_than(self.seen) {
            self.seen = Some(generation);
            self.gate.offer(proposal);
        }
        match self.gate.review() {
            GateVerdict::Accepted(proposal) => self.adopt(proposal),
            GateVerdict::NothingToReview
            | GateVerdict::Waiting
            | GateVerdict::RejectedStale
            | GateVerdict::RejectedForeignCamera => PlanRefresh::Unchanged,
        }
    }

    fn progress(&self) -> Option<AutoExposureProgress> {
        Some(match self.accepted.map(|proposal| proposal.convergence) {
            None => AutoExposureProgress::AwaitingMeasurement,
            Some(ProposalConvergence::Searching) => AutoExposureProgress::Searching,
            Some(ProposalConvergence::Settled) => AutoExposureProgress::Settled,
            Some(ProposalConvergence::Holding) => AutoExposureProgress::Holding,
        })
    }

    fn foreign_camera_proposals_rejected(&self) -> u64 {
        self.gate.foreign_camera_counter().load(Ordering::Relaxed)
    }

    fn reset_for_new_stream(&mut self) {
        if let Some(proposal) = self.accepted.take() {
            self.memory = Some(proposal.target);
        }
        self.driving_target = None;
        self.gate.reset_for_new_stream();
    }

    fn begin_stream_on_camera(&mut self, camera: &CameraId) {
        self.gate.begin_stream_on(CameraTag::of(camera));
    }

    fn carried_scene_target(&self) -> Option<ExposureProduct> {
        self.memory
    }

    fn scene_target_after_transfer(&self, transfer: &ExposureTransfer) -> Option<ExposureProduct> {
        // Тот же итог, что даст `reset_for_new_stream` + `carry_memory_to_new_camera`: последнее принятое
        // предложение прошлого потока, а если его не было, прежняя память.
        let remembered = self.accepted.map(|proposal| proposal.target).or(self.memory)?;
        Some(transfer.apply(remembered))
    }

    /// Память прошлого потока записана в произведении прошлой камеры: новой камере она нужна
    /// пересчитанной, иначе первая экспозиция смещена на разницу чувствительности (решение 126).
    fn carry_memory_to_new_camera(&mut self, transfer: &ExposureTransfer) {
        self.memory = self.memory.map(|remembered| transfer.apply(remembered));
    }
}
