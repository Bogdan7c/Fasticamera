//! Программируемые исполнители 3A для тестов: доказывают, что движок работает с **любой**
//! реализацией [`ExposureController`] / [`FocusController`], а не только с вендорской.
//!
//! Исполнитель уезжает в движок (`Box`), поэтому тест держит рядом разделяемую ручку
//! (`*Script`): через неё задаётся следующий план и читается, что движок исполнителю говорил.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::three_a::PointRequestId;

use crate::command::UnsupportedThreeA;
use crate::request_controls::{ExposurePlan, FocusPlan};
use crate::three_a_control::{
    ExposureController, ExposureDecision, FocusController, FocusDecision, FocusTriggerRequest, PlanRefresh,
    ThreeAContext, TriggerDelivery,
};
use crate::three_a_status::{ControlAdjustment, FocusOutcome, FocusProgress};

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
struct ExposureScript {
    plan: Result<ExposureDecision, UnsupportedThreeA>,
    refresh_reports_change: bool,
    plan_calls: u32,
    refresh_calls: u32,
    resets: u32,
    observed_frames: Vec<CaptureResultMeta>,
}

/// Ручка теста к [`ScriptedExposureController`].
#[derive(Debug, Clone)]
pub struct ScriptedExposure {
    shared: Arc<Mutex<ExposureScript>>,
}

/// Исполнитель экспозиции, отдающий заданный тестом план.
#[derive(Debug)]
pub struct ScriptedExposureController {
    shared: Arc<Mutex<ExposureScript>>,
}

impl ScriptedExposureController {
    /// Исполнитель с начальным планом и ручка к нему.
    pub fn new(initial_plan: ExposurePlan) -> (Self, ScriptedExposure) {
        let shared = Arc::new(Mutex::new(ExposureScript {
            plan: Ok(ExposureDecision {
                plan: initial_plan,
                adjustments: Vec::new(),
            }),
            refresh_reports_change: false,
            plan_calls: 0,
            refresh_calls: 0,
            resets: 0,
            observed_frames: Vec::new(),
        }));
        (
            Self {
                shared: Arc::clone(&shared),
            },
            ScriptedExposure { shared },
        )
    }
}

impl ScriptedExposure {
    /// Новое решение «со стороны»: следующий `refresh` сообщит `Changed` (один раз).
    pub fn publish_new_plan(&self, plan: ExposurePlan, adjustments: Vec<ControlAdjustment>) {
        let mut script = lock(&self.shared);
        script.plan = Ok(ExposureDecision { plan, adjustments });
        script.refresh_reports_change = true;
    }

    /// Следующие `plan` будут отвечать отказом.
    pub fn fail_planning(&self, error: UnsupportedThreeA) {
        lock(&self.shared).plan = Err(error);
    }

    pub fn plan_calls(&self) -> u32 {
        lock(&self.shared).plan_calls
    }

    pub fn refresh_calls(&self) -> u32 {
        lock(&self.shared).refresh_calls
    }

    pub fn resets(&self) -> u32 {
        lock(&self.shared).resets
    }

    pub fn observed_frames(&self) -> Vec<CaptureResultMeta> {
        lock(&self.shared).observed_frames.clone()
    }
}

impl ExposureController for ScriptedExposureController {
    fn plan(&mut self, _context: &ThreeAContext<'_>) -> Result<ExposureDecision, UnsupportedThreeA> {
        let mut script = lock(&self.shared);
        script.plan_calls += 1;
        script.plan.clone()
    }

    fn observe_result(&mut self, result: &CaptureResultMeta) {
        lock(&self.shared).observed_frames.push(*result);
    }

    fn refresh(&mut self) -> PlanRefresh {
        let mut script = lock(&self.shared);
        script.refresh_calls += 1;
        if std::mem::take(&mut script.refresh_reports_change) {
            PlanRefresh::Changed
        } else {
            PlanRefresh::Unchanged
        }
    }

    fn reset_for_new_stream(&mut self) {
        lock(&self.shared).resets += 1;
    }
}

#[derive(Debug)]
struct FocusScript {
    plan: FocusPlan,
    triggers: VecDeque<FocusTriggerRequest>,
    outcomes: VecDeque<(PointRequestId, FocusOutcome)>,
    deliveries: Vec<(FocusTriggerRequest, TriggerDelivery)>,
    timed_out_requests: Vec<PointRequestId>,
    observed_frames: Vec<CaptureResultMeta>,
    resets: u32,
    refresh_calls: u32,
    refresh_reports_change: bool,
}

/// Ручка теста к [`ScriptedFocusController`].
#[derive(Debug, Clone)]
pub struct ScriptedFocus {
    shared: Arc<Mutex<FocusScript>>,
}

/// Исполнитель фокуса, отдающий заданный тестом план, триггеры и итоги.
#[derive(Debug)]
pub struct ScriptedFocusController {
    shared: Arc<Mutex<FocusScript>>,
}

impl ScriptedFocusController {
    pub fn new(plan: FocusPlan) -> (Self, ScriptedFocus) {
        let shared = Arc::new(Mutex::new(FocusScript {
            plan,
            triggers: VecDeque::new(),
            outcomes: VecDeque::new(),
            deliveries: Vec::new(),
            timed_out_requests: Vec::new(),
            observed_frames: Vec::new(),
            resets: 0,
            refresh_calls: 0,
            refresh_reports_change: false,
        }));
        (
            Self {
                shared: Arc::clone(&shared),
            },
            ScriptedFocus { shared },
        )
    }
}

impl ScriptedFocus {
    /// Новый регион «со стороны» (как от трекера объекта): следующий `refresh` сообщит
    /// `Changed` (один раз), а `plan` отдаст этот план.
    pub fn publish_tracked_plan(&self, plan: FocusPlan) {
        let mut script = lock(&self.shared);
        script.plan = plan;
        script.refresh_reports_change = true;
    }

    pub fn refresh_calls(&self) -> u32 {
        lock(&self.shared).refresh_calls
    }

    /// `plan` отдаёт этот триггер, пока движок не подтвердит его отправку.
    pub fn queue_trigger(&self, trigger: FocusTriggerRequest) {
        lock(&self.shared).triggers.push_back(trigger);
    }

    /// Следующий `observe_result` вернёт этот итог.
    pub fn queue_outcome(&self, request: PointRequestId, outcome: FocusOutcome) {
        lock(&self.shared).outcomes.push_back((request, outcome));
    }

    pub fn deliveries(&self) -> Vec<(FocusTriggerRequest, TriggerDelivery)> {
        lock(&self.shared).deliveries.clone()
    }

    pub fn timed_out_requests(&self) -> Vec<PointRequestId> {
        lock(&self.shared).timed_out_requests.clone()
    }

    pub fn observed_frames(&self) -> Vec<CaptureResultMeta> {
        lock(&self.shared).observed_frames.clone()
    }

    pub fn resets(&self) -> u32 {
        lock(&self.shared).resets
    }
}

impl FocusController for ScriptedFocusController {
    fn plan(&mut self, _context: &ThreeAContext<'_>) -> Result<FocusDecision, UnsupportedThreeA> {
        let script = lock(&self.shared);
        // Триггер держится, пока движок не подтвердит отправку (`trigger_submitted`).
        let trigger = script.triggers.front().copied();
        Ok(FocusDecision {
            plan: script.plan,
            trigger,
            adjustments: Vec::new(),
        })
    }

    fn trigger_submitted(&mut self, request: FocusTriggerRequest, delivery: TriggerDelivery) {
        let mut script = lock(&self.shared);
        if script.triggers.front() == Some(&request) {
            script.triggers.pop_front();
        }
        script.deliveries.push((request, delivery));
    }

    fn observe_result(&mut self, result: &CaptureResultMeta) -> Option<(PointRequestId, FocusOutcome)> {
        let mut script = lock(&self.shared);
        script.observed_frames.push(*result);
        script.outcomes.pop_front()
    }

    fn refresh(&mut self) -> PlanRefresh {
        let mut script = lock(&self.shared);
        script.refresh_calls += 1;
        if std::mem::take(&mut script.refresh_reports_change) {
            PlanRefresh::Changed
        } else {
            PlanRefresh::Unchanged
        }
    }

    fn settle_timed_out(&mut self, request: PointRequestId) -> Option<FocusOutcome> {
        lock(&self.shared).timed_out_requests.push(request);
        Some(FocusOutcome::TimedOut)
    }

    fn progress(&self) -> FocusProgress {
        FocusProgress::Continuous { af_state: None }
    }

    fn reset_for_new_stream(&mut self) {
        lock(&self.shared).resets += 1;
    }
}
