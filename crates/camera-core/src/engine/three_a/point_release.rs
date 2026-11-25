//! Снятие точки тапа движком (решение 241): единственное место, где точка уходит без команды человека.
//!
//! Владелец точки — движок (`Desired::three_a`). Рендер лишь сообщает, что сцена под точкой сменилась или регион
//! ушёл за кадр (`PointObsolete`), а возврат из фона движок видит сам. Снять точку можно, только если её не держит
//! блокировка AE/AF и правило [`PointHold`] это разрешает.
//!
//! Это НЕ `decide_reset_point_of_interest`: тот снимает и блокировку (команда человека «сбросить»). Здесь
//! блокировка остаётся нетронутой, а заблокированную точку не снимаем вовсе.

use camera_contract::auto_exposure::CameraTag;
use camera_contract::scene_change::PointObsolete;
use camera_contract::three_a::{AeAfLock, DirectiveGeneration, PointOfInterest, PointRequestId};

use super::super::{CameraEngine, LOG_TARGET};
use super::ThreeAEngine;
use crate::event::CameraEvent;
use crate::point_hold::{PointHold, PointReleaseCause};

/// Что вызвало попытку снять точку.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::engine) enum PointReleaseTrigger {
    /// Наблюдение рендера: точка `request` устарела на кадре камеры `measured_on`.
    Observation(PointObsolete),
    /// Приложение вернулось из фона: снимается точка, какая бы она ни была (номера у возврата нет).
    ReturnedToApp,
}

/// Итог попытки снять точку. Исходы различимы: вызывающему важно, снята точка или почему нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum PointReleaseOutcome {
    /// Точка снята: замер и фокус по всему кадру, блокировка не менялась (она была снята).
    Released {
        request: PointRequestId,
        cause: PointReleaseCause,
    },
    /// Наблюдение про другой тап (опоздавшее сообщение о прошлой точке): текущая точка не тронута.
    OtherRequest,
    /// Точку держит блокировка AE/AF.
    HeldByLock,
    /// Правило конфига `until_replaced`: точка живёт до нового тапа.
    HoldPolicy,
    /// Точки нет (кадр целиком): снимать нечего.
    NoPoint,
    /// Наблюдение сделано на другой камере, чем у текущего потока (или потока ещё нет).
    ForeignCamera,
}

impl ThreeAEngine {
    /// Наблюдение «точка устарела» новее уже разобранного. Чтение НЕ сдвигает отметку: её сдвигает
    /// [`Self::mark_point_obsolete_seen`] после разбора, чтобы не потерять наблюдение, которое пока читать нельзя.
    pub(in crate::engine) fn point_obsolete_unseen(&self) -> Option<(DirectiveGeneration, PointObsolete)> {
        self.exposure.point_obsolete_newer_than(self.point_obsolete_seen)
    }

    /// Наблюдение поколения `generation` разобрано: повторно его не читаем.
    pub(in crate::engine) fn mark_point_obsolete_seen(&mut self, generation: DirectiveGeneration) {
        self.point_obsolete_seen = Some(generation);
    }

    /// Камера текущего потока: наблюдение чужой камеры точку не снимает.
    fn is_stream_camera(&self, camera: CameraTag) -> bool {
        self.exposure_camera
            .as_ref()
            .is_some_and(|stream_camera| CameraTag::of(stream_camera) == camera)
    }
}

impl CameraEngine {
    /// Попытаться снять точку. Единственная точка входа снятия без команды: меняет только `point`, блокировку
    /// не трогает (при `Unlocked` она и так снята). Событие, счётчик, журнал и перестройку запроса делает
    /// вызывающий через [`Self::announce_point_release`].
    pub(in crate::engine) fn release_point(&mut self, trigger: PointReleaseTrigger) -> PointReleaseOutcome {
        let outcome = self.decide_point_release(trigger);
        if matches!(outcome, PointReleaseOutcome::Released { .. }) {
            self.desired.three_a.point = PointOfInterest::WholeFrame;
        }
        outcome
    }

    /// Решение без изменения состояния: порядок проверок задаёт, какая причина отказа названа.
    fn decide_point_release(&self, trigger: PointReleaseTrigger) -> PointReleaseOutcome {
        if self.config.three_a.point_hold == PointHold::UntilReplaced {
            return PointReleaseOutcome::HoldPolicy;
        }
        let PointOfInterest::Point { request, .. } = self.desired.three_a.point else {
            return PointReleaseOutcome::NoPoint;
        };
        let cause = match trigger {
            PointReleaseTrigger::Observation(observation) => {
                if !self.three_a.is_stream_camera(observation.measured_on) {
                    return PointReleaseOutcome::ForeignCamera;
                }
                if observation.request != request {
                    return PointReleaseOutcome::OtherRequest;
                }
                PointReleaseCause::from(observation.cause)
            }
            PointReleaseTrigger::ReturnedToApp => PointReleaseCause::ReturnedToApp,
        };
        if self.desired.three_a.lock == AeAfLock::Locked {
            return PointReleaseOutcome::HeldByLock;
        }
        PointReleaseOutcome::Released { request, cause }
    }

    /// Учесть итог попытки: счётчики, журнал и событие. `true` — точка снята и запрос надо перестроить.
    pub(in crate::engine) fn announce_point_release(&mut self, outcome: PointReleaseOutcome) -> bool {
        match outcome {
            PointReleaseOutcome::Released { request, cause } => {
                self.three_a.counters.points_released += 1;
                tracing::info!(target: LOG_TARGET, request = request.0, ?cause, "точка тапа отпущена движком: замер и фокус по всему кадру");
                self.events
                    .push(CameraEvent::PointOfInterestReleased { request, cause });
                true
            }
            other => {
                self.three_a.counters.point_obsolete_ignored += 1;
                tracing::debug!(target: LOG_TARGET, ?other, "наблюдение «точка устарела» не применено");
                false
            }
        }
    }

    /// Разобрать наблюдение рендера, если читать его сейчас можно. Пока идёт снимок со вспышкой или серия,
    /// наблюдение НЕ читается и не теряется: отметка не сдвигается, его прочтёт следующее пробуждение
    /// (иначе точка сменилась бы посреди снимка, а экспозицию тогда держит снимок). `true` — точка снята.
    pub(in crate::engine) fn poll_point_obsolete(&mut self) -> bool {
        if self.flash.in_progress() || self.three_a.bracket_in_progress() {
            return false;
        }
        let Some((generation, observation)) = self.three_a.point_obsolete_unseen() else {
            return false;
        };
        self.three_a.mark_point_obsolete_seen(generation);
        let outcome = self.release_point(PointReleaseTrigger::Observation(observation));
        self.announce_point_release(outcome)
    }

    /// Возврат из фона: незаблокированная точка снимается. Запрос перестроит вызывающий (`reconcile`).
    pub(in crate::engine) fn release_point_on_return_to_app(&mut self) {
        let outcome = self.release_point(PointReleaseTrigger::ReturnedToApp);
        // Без точки или при блокировке «не применено» считать нечем: возврат из фона — не наблюдение.
        if matches!(outcome, PointReleaseOutcome::Released { .. }) {
            self.announce_point_release(outcome);
        }
    }
}
