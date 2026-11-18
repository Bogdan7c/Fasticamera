//! Серия с брекетингом: отправка, сбор результатов по номерам кадров, итог.
//!
//! Серия — разовое действие, а не уровневое желаемое состояние: она не переживает потерю
//! сессии (итог `SessionLost`), и «повторить» её после восстановления решает вызывающий.
//! Одновременно идёт одна серия.

use std::time::Instant;

use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::three_a::{BurstId, ExposureBracket};

use super::super::{CameraEngine, LOG_TARGET};
use crate::backend::RequestFailure;
use crate::event::CameraEvent;
use crate::three_a_status::{BracketFailure, BracketFrameResult, BracketReport};

/// Идущая серия.
pub(super) struct BracketRun {
    burst: BurstId,
    bracket: ExposureBracket,
    /// Результаты по индексу кадра серии; `None` — ещё не пришёл.
    frames: Vec<Option<BracketFrameResult>>,
    /// Срок ожидания результатов; `None`, пока серия принята, но не отправлена.
    deadline: Option<Instant>,
}

impl BracketRun {
    /// Серия принята командой, отправка впереди.
    pub(super) fn accepted(burst: BurstId, bracket: ExposureBracket) -> Self {
        let frames = vec![None; bracket.frames().len()];
        Self {
            burst,
            bracket,
            frames,
            deadline: None,
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Пришедшие кадры в порядке индекса.
    fn received_in_order(&self) -> Vec<BracketFrameResult> {
        self.frames.iter().flatten().copied().collect()
    }

    fn is_complete(&self) -> bool {
        self.frames.iter().all(Option::is_some)
    }
}

impl CameraEngine {
    /// Отправить принятую командой серию бэкенду.
    pub(in crate::engine) fn submit_pending_bracket(&mut self, now: Instant) {
        let Some(applied) = self.session.as_ref().and_then(|session| session.repeating) else {
            self.fail_bracket(BracketFailure::SessionLost);
            return;
        };
        let Some(run) = &self.three_a.bracket else {
            return;
        };
        match self
            .backend
            .submit_bracket(applied.intent, &applied.controls, &run.bracket, run.burst)
        {
            Ok(()) => {
                self.three_a.counters.brackets_submitted += 1;
                let timeout = self.bracket_result_timeout();
                if let Some(run) = &mut self.three_a.bracket {
                    run.deadline = Some(now + timeout);
                }
            }
            Err(failure) => {
                tracing::warn!(target: LOG_TARGET, %failure, "серия не принята бэкендом");
                self.fail_bracket(BracketFailure::SubmitFailed);
                // Потеря устройства — общая беда сессии; отказ самой серии (например, HAL не
                // принял число запросов) сессию не ломает и превращается только в итог серии.
                if !matches!(failure, RequestFailure::Backend(_)) {
                    let _ = self.fail_from_request_failure(failure, now);
                }
            }
        }
    }

    /// Результат кадра серии: сохранить, а когда пришли все — объявить итог.
    pub(in crate::engine) fn collect_bracket_frame(&mut self, burst: BurstId, index: u32, result: &CaptureResultMeta) {
        let Some(run) = &mut self.three_a.bracket else {
            return;
        };
        if run.burst != burst {
            return;
        }
        // Индекс вне серии или повторный результат того же кадра — не наш кадр, не затираем.
        let Some(slot) = usize::try_from(index)
            .ok()
            .and_then(|position| run.frames.get_mut(position))
        else {
            return;
        };
        if slot.is_some() {
            return;
        }
        *slot = Some(BracketFrameResult {
            index,
            frame_number: result.frame_number,
            sensor_timestamp: result.sensor_timestamp,
            exposure_time: result.exposure_time,
            sensitivity_iso: result.sensitivity_iso,
        });
        if run.is_complete() {
            self.finish_bracket_with_success();
        }
    }

    fn finish_bracket_with_success(&mut self) {
        let Some(run) = self.three_a.bracket.take() else {
            return;
        };
        self.three_a.counters.brackets_completed += 1;
        self.events.push(CameraEvent::BracketCaptured {
            report: BracketReport::Captured {
                burst: run.burst,
                frames: run.received_in_order(),
            },
        });
    }

    /// Серия не удалась: итог с тем, что успело прийти. Нет серии — ничего не делает.
    pub(in crate::engine) fn fail_bracket(&mut self, failure: BracketFailure) {
        let Some(run) = self.three_a.bracket.take() else {
            return;
        };
        self.three_a.counters.bracket_failures += 1;
        self.events.push(CameraEvent::BracketCaptured {
            report: BracketReport::Failed {
                burst: run.burst,
                failure,
                frames: run.received_in_order(),
            },
        });
    }
}
