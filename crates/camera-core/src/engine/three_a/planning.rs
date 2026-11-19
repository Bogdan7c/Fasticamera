//! План управляющей части запроса: чистое объединение планов исполнителей и применение.
//!
//! Порядок: сначала экспозиция, потом фокус. Исполнитель фокуса помнит выданный триггер, и
//! отказ экспозиции не должен «съесть» его раньше, чем план дойдёт до бэкенда.
//!
//! Отказ исполнителя при планировании **в `reconcile`** (команды, которая ждала бы ответа,
//! нет — например, после смены камеры на слабую) не останавливает движок и не зацикливается:
//! * экспозиция: желаемые ручные значения остаются в намерении, а запрос строится как автомат
//!   с явными поправками `NotSupportedByCamera`;
//! * фокус: непрерывный AF без точки, предупреждение в журнал.

use std::time::Instant;

use camera_contract::auto_exposure::ProductReach;
use camera_contract::capabilities::CameraCapabilities;
use camera_contract::control::{CaptureIntent, ManualControl};

use super::super::{CameraEngine, Halt, LOG_TARGET};
use super::ThreeAEngine;
use super::exposure_directive::applied_from_plan;
use crate::backend::RequestFailure;
use crate::command::UnsupportedThreeA;
use crate::event::CameraEvent;
use crate::request_controls::{ExposurePlan, FocusPlan, RequestControls};
use crate::three_a_control::{
    ExposureDecision, FocusDecision, FocusTriggerRequest, ThreeAContext, ThreeAIntent, TriggerDelivery,
};
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter, FocusOutcome};
use camera_contract::three_a::AeAfLock;

/// План одного запроса: что поставить в запрос, нужен ли триггер и какие значения поправлены.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::engine) struct PlannedRequest {
    pub(in crate::engine) controls: RequestControls,
    pub(in crate::engine) trigger: Option<FocusTriggerRequest>,
    pub(in crate::engine) adjustments: Vec<ControlAdjustment>,
    /// Достижимые произведения экспозиции этого плана (для указания рендеру).
    pub(in crate::engine) exposure_reach: ProductReach,
}

/// Чистое объединение планов исполнителей в план запроса.
pub(super) fn combine_plans(exposure: ExposureDecision, focus: FocusDecision) -> PlannedRequest {
    let exposure_reach = exposure.reach;
    let mut adjustments = exposure.adjustments;
    adjustments.extend(focus.adjustments);
    PlannedRequest {
        controls: RequestControls {
            exposure: exposure.plan,
            focus: focus.plan,
        },
        trigger: focus.trigger,
        adjustments,
        exposure_reach,
    }
}

/// Экспозиция, когда исполнитель отказал, а ответить некому: автомат, ручные значения —
/// в поправки. `applied = 0` значит «этот параметр ведёт автомат».
pub(super) fn degraded_exposure(controls: &ManualControl) -> ExposureDecision {
    let mut adjustments = Vec::new();
    if let Some(iso) = controls.iso.manual_value() {
        adjustments.push(not_supported(ControlParameter::Iso, f64::from(iso.value())));
    }
    if let Some(shutter) = controls.shutter.manual_value() {
        adjustments.push(not_supported(
            ControlParameter::Shutter,
            shutter.duration().as_nanos() as f64,
        ));
    }
    ExposureDecision {
        plan: ExposurePlan::VendorAuto {
            compensation_steps: 0,
            lock: AeAfLock::Unlocked,
            region: None,
        },
        adjustments,
        reach: ProductReach::Unbounded,
    }
}

fn not_supported(parameter: ControlParameter, requested: f64) -> ControlAdjustment {
    ControlAdjustment {
        parameter,
        requested,
        applied: 0.0,
        reason: AdjustmentReason::NotSupportedByCamera,
    }
}

impl ThreeAEngine {
    /// План запроса; отказы исполнителей смягчаются, как описано в шапке модуля.
    pub(super) fn plan(&mut self, context: &ThreeAContext<'_>) -> PlannedRequest {
        let exposure = self.exposure.plan(context).unwrap_or_else(|error| {
            tracing::warn!(target: LOG_TARGET, ?error, "исполнитель экспозиции отказал: строим автомат");
            degraded_exposure(&context.intent.controls)
        });
        let focus = self.focus.plan(context).unwrap_or_else(|error| {
            tracing::warn!(target: LOG_TARGET, ?error, "исполнитель фокуса отказал: непрерывный AF");
            FocusDecision {
                plan: FocusPlan::Continuous { region: None },
                trigger: None,
                adjustments: Vec::new(),
            }
        });
        combine_plans(exposure, focus)
    }

    /// Выполнима ли команда с этим намерением. Нужна, чтобы ответить на команду
    /// `Unsupported`; сам запрос потом строит [`ThreeAEngine::plan`].
    ///
    /// Исполнитель фокуса при этом может отметить новый тап у себя, но триггер он держит,
    /// пока движок не подтвердит отправку, поэтому следующий `plan` отдаст его снова, и
    /// никакой план не пропадает. Экспозиция спрашивается первой: отказ возвращается раньше,
    /// чем исполнитель фокуса тронул своё состояние.
    pub(super) fn validate(&mut self, context: &ThreeAContext<'_>) -> Result<(), UnsupportedThreeA> {
        self.exposure.plan(context)?;
        self.focus.plan(context)?;
        Ok(())
    }
}

/// Значение, которым считаем длительность кадра, когда режима нет (HAL не ограничивает).
fn unconstrained_frame_duration() -> camera_contract::capabilities::FrameDuration {
    camera_contract::capabilities::FrameDuration::from_nanos(0)
}

impl CameraEngine {
    /// Возможности камеры, на которой строится план: открытая, иначе желаемая.
    pub(super) fn planning_capabilities(&self) -> Option<CameraCapabilities> {
        let camera = match &self.device {
            Some(device) => device.camera.clone(),
            None => self.desired_camera_id()?,
        };
        self.catalog
            .camera(&camera)
            .map(|descriptor| descriptor.capabilities.clone())
    }

    /// Контекст планирования для намерения `intent` и вызов `with` с ним. `None` — камеры нет.
    fn with_three_a_context<T>(
        &mut self,
        intent: &ThreeAIntent,
        with: impl FnOnce(&mut ThreeAEngine, &ThreeAContext<'_>) -> T,
    ) -> Option<T> {
        let capabilities = self.planning_capabilities()?;
        let context = ThreeAContext {
            intent,
            capture_intent: self.desired.streaming.unwrap_or(CaptureIntent::Preview),
            capabilities: &capabilities,
            stream_frame_duration: self
                .desired
                .mode
                .map_or_else(unconstrained_frame_duration, |mode| mode.frame_rate.frame_duration()),
        };
        Some(with(&mut self.three_a, &context))
    }

    /// Проверить кандидата-намерение перед принятием команды.
    pub(super) fn validate_three_a_intent(&mut self, candidate: &ThreeAIntent) -> Result<(), UnsupportedThreeA> {
        // Камеры ещё нет — проверить нечем; план проверит `reconcile` (с мягким отказом).
        self.with_three_a_context(candidate, ThreeAEngine::validate)
            .unwrap_or(Ok(()))
    }

    /// План для повторяющегося запроса по текущему желаемому намерению.
    pub(in crate::engine) fn plan_three_a(&mut self) -> PlannedRequest {
        let intent = self.desired.three_a;
        self.with_three_a_context(&intent, ThreeAEngine::plan)
            .unwrap_or_else(|| {
                tracing::error!(target: LOG_TARGET, "нет возможностей камеры для плана 3A: строим автомат");
                combine_plans(
                    degraded_exposure(&intent.controls),
                    FocusDecision {
                        plan: FocusPlan::Continuous { region: None },
                        trigger: None,
                        adjustments: Vec::new(),
                    },
                )
            })
    }

    /// После того как повторяющийся запрос соответствует плану: объявить поправки и
    /// отправить триггер тапа.
    pub(in crate::engine) fn finish_planned_request(
        &mut self,
        planned: PlannedRequest,
        now: Instant,
    ) -> Result<(), Halt> {
        // Повторяющийся запрос уже стоит: рендеру можно сказать, что в нём (и где мерить).
        let applied = applied_from_plan(&planned.controls.exposure, &planned.adjustments, planned.exposure_reach);
        self.three_a
            .exposure_directive
            .publish_if_changed(&self.desired.three_a, applied);
        self.announce_adjustments(planned.adjustments);
        let Some(trigger) = planned.trigger else {
            return Ok(());
        };
        let Some(applied) = self.session.as_ref().and_then(|session| session.repeating) else {
            return Ok(());
        };
        match self
            .backend
            .submit_focus_trigger(applied.intent, &applied.controls, trigger.trigger, trigger.request)
        {
            Ok(()) => {
                self.three_a.counters.focus_triggers_sent += 1;
                self.three_a
                    .focus
                    .trigger_submitted(trigger, TriggerDelivery::Submitted);
                let timeout = std::time::Duration::from_millis(self.config.three_a.focus_settle_timeout_ms);
                self.three_a.focus_settle_deadline = Some((trigger.request, now + timeout));
                Ok(())
            }
            Err(failure) => {
                // Политика как у серии: потеря устройства или сессии — общая беда и уходит в
                // восстановление; отказ самого разового запроса (`Backend`) сессию не ломает —
                // поток идёт дальше, а тап получает явный итог, а не молчание.
                tracing::warn!(target: LOG_TARGET, %failure, "запрос с триггером фокуса не принят");
                self.three_a.counters.focus_trigger_failures += 1;
                self.three_a.focus.trigger_submitted(trigger, TriggerDelivery::Failed);
                self.events.push(CameraEvent::FocusSettled {
                    request: trigger.request,
                    outcome: FocusOutcome::TriggerNotDelivered,
                });
                if matches!(failure, RequestFailure::Backend(_)) {
                    Ok(())
                } else {
                    Err(self.fail_from_request_failure(failure, now))
                }
            }
        }
    }

    /// Поправки в снимок; событие — только если список не пуст и отличается от объявленного.
    fn announce_adjustments(&mut self, adjustments: Vec<ControlAdjustment>) {
        if adjustments != self.three_a.announced_adjustments {
            if !adjustments.is_empty() {
                self.events.push(CameraEvent::ControlsAdjusted {
                    adjustments: adjustments.clone(),
                });
            }
            self.three_a.announced_adjustments = adjustments.clone();
        }
        self.three_a.adjustments = adjustments;
    }
}
