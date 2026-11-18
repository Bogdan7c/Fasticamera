//! Вендорский автофокус через HAL и машина «тап → скан → итог».
//!
//! Режимы по намерению:
//! * `Fixed` у камеры (фронт OnePlus 11) — `FixedLens`, ключи AF в запрос не идут; тап меняет
//!   только замер экспозиции;
//! * ручная дистанция — `Manual`, дистанция подтягивается к `[0, ближайший фокус]`; тап на
//!   ручном фокусе фокус не двигает (как и в iOS: ручной фокус сильнее тапа);
//! * без точки — непрерывный AF;
//! * точка — `AF_MODE_AUTO` и один триггер `Start` на номер тапа. Блокировка AE/AF держит
//!   тот же `AF_MODE_AUTO`: после `FOCUSED_LOCKED` объектив стоит, пока режим не сменят.
//!
//! Уход из однократного режима в непрерывный отдельного `Cancel` не требует: смена
//! `AF_MODE` сама сбрасывает состояние AF (документация Camera2, `CONTROL_AF_TRIGGER`).
//!
//! # Как отличить результат тапа от устаревшего
//!
//! Конвейер Camera2 держит несколько кадров в полёте, поэтому сразу после триггера ещё
//! приходят результаты **до** него, а в них нередко `FOCUSED_LOCKED` от прошлого тапа.
//! Началом отсчёта служит результат самого триггера (`RequestOrigin::FocusTrigger`), и
//! верим только результатам с `frame_number` не меньше номера его кадра. Номер кадра, а не
//! порядок прихода: разовые результаты идут по невытесняемой полосе ящика и могут обгонять
//! старые результаты потока. Если метка триггера не пришла (бэкенд не опознал запрос),
//! после `focus_marker_grace_frames` результатов потока верим состоянию AF как есть —
//! тогда лучше возможно ранний итог, чем тап, который вечно ждёт таймера.
//!
//! В RAW-only сессии `AF_STATE` не приходит вовсе (решение 62): тап закончится только
//! таймером `TimedOut`, и это честный итог.

use camera_contract::capabilities::FocusCapability;
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber};
use camera_contract::control::FocusDistance;
use camera_contract::three_a::{AfModeSupport, AfState, PointOfInterest, PointRequestId, RequestOrigin};

use crate::command::UnsupportedThreeA;
use crate::request_controls::{AfTrigger, FocusPlan};
use crate::three_a_control::{
    FocusController, FocusDecision, FocusTriggerRequest, PlanRefresh, ThreeAContext, TriggerDelivery,
};
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter, FocusOutcome, FocusProgress};

/// Откуда доверять результатам тапа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustFrom {
    /// Метки триггера ещё нет; сколько результатов потока прошло с отправки.
    WaitingForMarker { stream_results_seen: u32 },
    /// Кадр метки известен: верим результатам с номером не меньше.
    Frame(FrameNumber),
    /// Метка потеряна, льготные кадры вышли: верим всему подряд.
    Anything,
}

/// Тап, ожидающий итога.
#[derive(Debug, Clone, Copy)]
struct AwaitedSettle {
    request: PointRequestId,
    trust: TrustFrom,
}

/// Вендорский AF.
#[derive(Debug)]
pub struct VendorFocus {
    progress: FocusProgress,
    /// Тап, для которого триггер выдан и подтверждён движком (`trigger_submitted`).
    submitted: Option<PointRequestId>,
    awaiting: Option<AwaitedSettle>,
    /// `three_a.focus_marker_grace_frames`.
    marker_grace_frames: u32,
}

impl VendorFocus {
    pub fn new(marker_grace_frames: u32) -> Self {
        Self {
            progress: FocusProgress::Continuous { af_state: None },
            submitted: None,
            awaiting: None,
            marker_grace_frames,
        }
    }

    /// Вышли из однократного режима (или он не нужен): забыть тап и его ожидание.
    fn forget_tap(&mut self) {
        self.submitted = None;
        self.awaiting = None;
    }

    fn plan_manual(&mut self, requested: FocusDistance, nearest_diopters: f32) -> FocusDecision {
        self.forget_tap();
        self.progress = FocusProgress::Manual;
        let mut adjustments = Vec::new();
        let applied = if requested.diopters() > nearest_diopters {
            // Ближе минимальной дистанции фокуса камера сфокусироваться не может.
            adjustments.push(ControlAdjustment {
                parameter: ControlParameter::Focus,
                requested: f64::from(requested.diopters()),
                applied: f64::from(nearest_diopters),
                reason: AdjustmentReason::AboveCameraRange,
            });
            FocusDistance::from_diopters(nearest_diopters).unwrap_or(FocusDistance::INFINITY)
        } else {
            requested
        };
        FocusDecision {
            plan: FocusPlan::Manual { distance: applied },
            trigger: None,
            adjustments,
        }
    }

    fn plan_single_shot(
        &mut self,
        region: camera_contract::three_a::MeteringRegion,
        request: PointRequestId,
    ) -> FocusDecision {
        let trigger = (self.submitted != Some(request)).then_some(FocusTriggerRequest {
            trigger: AfTrigger::Start,
            request,
        });
        if trigger.is_some() {
            // Новый тап: прошлый итог устарел, ждём итог этого (повторный `plan` до подтверждения
            // триггера даёт тот же результат).
            self.awaiting = None;
            self.progress = FocusProgress::Scanning { request };
        }
        FocusDecision {
            plan: FocusPlan::SingleShot { region },
            trigger,
            adjustments: Vec::new(),
        }
    }

    fn plan_continuous(&mut self) -> FocusDecision {
        let was_single_shot = self.submitted.is_some() || self.awaiting.is_some();
        self.forget_tap();
        if was_single_shot || !matches!(self.progress, FocusProgress::Continuous { .. }) {
            // Состояние AF появится с первым результатом уже непрерывного режима: прежнее
            // относилось к однократному и только вводило бы в заблуждение.
            self.progress = FocusProgress::Continuous { af_state: None };
        }
        FocusDecision {
            plan: FocusPlan::Continuous { region: None },
            trigger: None,
            adjustments: Vec::new(),
        }
    }

    /// Итог тапа по состоянию AF, если оно итоговое.
    fn outcome_of(af_state: Option<AfState>) -> Option<FocusOutcome> {
        match af_state? {
            AfState::FocusedLocked => Some(FocusOutcome::Focused),
            AfState::NotFocusedLocked => Some(FocusOutcome::NotFocused),
            AfState::Inactive
            | AfState::PassiveScan
            | AfState::PassiveFocused
            | AfState::PassiveUnfocused
            | AfState::ActiveScan => None,
        }
    }
}

impl FocusController for VendorFocus {
    fn plan(&mut self, context: &ThreeAContext<'_>) -> Result<FocusDecision, UnsupportedThreeA> {
        let manual = context.intent.controls.focus.manual_value();
        let nearest_diopters = match context.capabilities.focus {
            FocusCapability::Fixed => {
                self.forget_tap();
                self.progress = FocusProgress::FixedLens;
                let adjustments = manual
                    .map(|requested| ControlAdjustment {
                        parameter: ControlParameter::Focus,
                        requested: f64::from(requested.diopters()),
                        applied: 0.0,
                        reason: AdjustmentReason::NotSupportedByCamera,
                    })
                    .into_iter()
                    .collect();
                return Ok(FocusDecision {
                    plan: FocusPlan::FixedLens,
                    trigger: None,
                    adjustments,
                });
            }
            FocusCapability::Adjustable {
                min_focus_distance_diopters,
                ..
            } => min_focus_distance_diopters,
        };
        if let Some(requested) = manual {
            return Ok(self.plan_manual(requested, nearest_diopters));
        }
        match context.intent.point {
            PointOfInterest::WholeFrame => Ok(self.plan_continuous()),
            PointOfInterest::Point { region, request } => {
                if !context.capabilities.three_a.supports_af_mode(AfModeSupport::Auto) {
                    return Err(UnsupportedThreeA::SingleShotFocus);
                }
                Ok(self.plan_single_shot(region, request))
            }
        }
    }

    fn trigger_submitted(&mut self, request: FocusTriggerRequest, delivery: TriggerDelivery) {
        self.submitted = Some(request.request);
        match delivery {
            TriggerDelivery::Submitted => {
                self.awaiting = Some(AwaitedSettle {
                    request: request.request,
                    trust: TrustFrom::WaitingForMarker { stream_results_seen: 0 },
                });
                self.progress = FocusProgress::Scanning {
                    request: request.request,
                };
            }
            TriggerDelivery::Failed => {
                // Триггер не дошёл, скана не будет; повторять его сами не пытаемся — движок
                // объявит итог `TriggerNotDelivered`, новый тап даст новую попытку.
                self.awaiting = None;
                self.progress = FocusProgress::Settled {
                    request: request.request,
                    outcome: FocusOutcome::TriggerNotDelivered,
                };
            }
        }
    }

    fn observe_result(&mut self, result: &CaptureResultMeta) -> Option<(PointRequestId, FocusOutcome)> {
        let state = &result.three_a;
        if let FocusProgress::Continuous { .. } = self.progress {
            self.progress = FocusProgress::Continuous {
                af_state: state.af_state,
            };
        }
        let mut awaited = self.awaiting?;
        awaited.trust = match (awaited.trust, state.origin) {
            (_, RequestOrigin::FocusTrigger { request }) if request == awaited.request => {
                TrustFrom::Frame(result.frame_number)
            }
            (TrustFrom::WaitingForMarker { stream_results_seen }, RequestOrigin::Repeating) => {
                if stream_results_seen >= self.marker_grace_frames {
                    TrustFrom::Anything
                } else {
                    TrustFrom::WaitingForMarker {
                        stream_results_seen: stream_results_seen + 1,
                    }
                }
            }
            (trust, _) => trust,
        };
        self.awaiting = Some(awaited);
        let trusted = match awaited.trust {
            TrustFrom::WaitingForMarker { .. } => false,
            TrustFrom::Frame(marker) => result.frame_number >= marker,
            TrustFrom::Anything => true,
        };
        if !trusted {
            return None;
        }
        let outcome = Self::outcome_of(state.af_state)?;
        self.awaiting = None;
        self.progress = FocusProgress::Settled {
            request: awaited.request,
            outcome,
        };
        Some((awaited.request, outcome))
    }

    fn refresh(&mut self) -> PlanRefresh {
        // Вендорский AF ведёт регион только по намерению движка; внешнего источника нет.
        PlanRefresh::Unchanged
    }

    fn settle_timed_out(&mut self, request: PointRequestId) -> Option<FocusOutcome> {
        let awaited = self.awaiting.filter(|awaited| awaited.request == request)?;
        self.awaiting = None;
        self.progress = FocusProgress::Settled {
            request: awaited.request,
            outcome: FocusOutcome::TimedOut,
        };
        Some(FocusOutcome::TimedOut)
    }

    fn progress(&self) -> FocusProgress {
        self.progress
    }

    fn reset_for_new_stream(&mut self) {
        self.forget_tap();
        self.progress = FocusProgress::Continuous { af_state: None };
    }
}
