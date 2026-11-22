//! Вторая половина последовательности: пауза без фонарика, разовый запрос со вспышкой, результат,
//! сроки, выходы и единственный итог.
//!
//! Правила выходов:
//! * любой итог проходит через [`CameraEngine::finish_flash_run`]: забирает последовательность одним
//!   `take()` и больше её не видит, поэтому повторный вход (например, `reconcile` убил поток и дошёл
//!   до `end_three_a_stream`) не создаст второго итога;
//! * выход при живом потоке (`fail_flash_keeping_stream`, успех) перестраивает повторяющийся запрос без
//!   фонарика и догоняет решения нашего AE, которые копились, пока последовательность шла;
//! * выход при потерянном потоке (`fail_flash_stream_ended`) ничего не перестраивает: запроса больше нет,
//!   а следующий будет построен уже без фонарика.

use std::time::{Duration, Instant};

use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::flash::{FlashControl, FlashShotId, FlashState, FlashStillFailure, PreflashMetering};

use super::super::{CameraEngine, LOG_TARGET};
use super::{FlashRun, Stage};
use crate::backend::RequestFailure;
use crate::event::CameraEvent;
use crate::flash_status::{FlashFrameResult, FlashStillReport};
use crate::request_controls::{ExposurePlan, RequestControls};

impl CameraEngine {
    /// Положить итог в события и показать наблюдателю: ровно в этом месте, ровно один раз на итог.
    pub(in crate::engine) fn emit_flash_report(&mut self, report: FlashStillReport) {
        self.events.push(CameraEvent::FlashStillFinished { report });
        if let Some(observer) = &self.flash.observer {
            observer.flash_still_finished(&report);
        }
    }

    /// Закрыть последовательность итогом. `None`, если её уже нет (итог уже был).
    fn finish_flash_run(
        &mut self,
        make_report: impl FnOnce(&FlashRun) -> FlashStillReport,
    ) -> Option<FlashStillReport> {
        let run = self.flash.run.take()?;
        let report = make_report(&run);
        match report {
            FlashStillReport::Fired { .. } => self.flash.counters.fired += 1,
            FlashStillReport::Failed { .. } => self.flash.counters.failed += 1,
        }
        // Предвспышки больше нет; план остаётся в указании: рендер запоминает его по номеру снимка.
        self.three_a
            .update_flash_directive(|directive| directive.preflash = PreflashMetering::Inactive);
        self.emit_flash_report(report);
        Some(report)
    }

    /// Снимок не удался, поток жив: итог, повторяющийся запрос без фонарика, догон AE.
    pub(in crate::engine) fn fail_flash_keeping_stream(&mut self, failure: FlashStillFailure, now: Instant) {
        let finished = self
            .finish_flash_run(|run| FlashStillReport::Failed {
                shot: run.shot,
                failure,
            })
            .is_some();
        if finished {
            self.resume_after_flash(now);
        }
    }

    /// Поток закончился (сессия потеряна, закрыта, объектив сменился): итог без перестройки запроса.
    ///
    /// Причина: камера желаемого намерения отличается от камеры, на которой шёл снимок, — объектив
    /// сменился (`LensChanged`, в том числе при повышении тёплой камеры); иначе сессия потеряна, закрыта
    /// или ушла в фон (`SessionLost`). Одно правило на все пути, которые сводятся в `end_three_a_stream`.
    pub(in crate::engine) fn fail_flash_stream_ended(&mut self) {
        let Some(run_camera) = self.flash.run.as_ref().map(|run| run.camera.clone()) else {
            return;
        };
        let moved = self.desired_camera_id().is_some_and(|camera| camera != run_camera);
        let failure = if moved {
            FlashStillFailure::LensChanged
        } else {
            FlashStillFailure::SessionLost
        };
        self.finish_flash_run(|run| FlashStillReport::Failed {
            shot: run.shot,
            failure,
        });
    }

    /// Вернуться к обычному запросу: наш AE догоняет решения, накопленные за время последовательности,
    /// и запрос пересобирается (без фонарика).
    fn resume_after_flash(&mut self, now: Instant) {
        let _ = self.three_a.catch_up_exposure();
        self.reconcile(now);
    }

    /// Сработали сроки последовательности.
    pub(in crate::engine) fn fire_flash_timers(&mut self, now: Instant) {
        let Some(stage) = self.flash.run.as_ref().map(|run| run.stage) else {
            return;
        };
        match stage {
            Stage::Preflash(preflash) if now >= preflash.deadline => {
                tracing::warn!(
                    target: LOG_TARGET,
                    remeasures = preflash.remeasures,
                    has_ambient = preflash.ambient.is_some(),
                    torch_frame_floor = ?preflash.torch_frame_floor,
                    "срок предвспышки вышел без годного замера"
                );
                self.fail_flash_keeping_stream(FlashStillFailure::PreflashNotMeasured, now);
            }
            Stage::Gap { submit_at, .. } if now >= submit_at => self.submit_flash_still_request(now),
            Stage::Fired { deadline, .. } if now >= deadline => {
                self.fail_flash_keeping_stream(FlashStillFailure::ResultTimedOut, now);
            }
            Stage::Preflash(_) | Stage::Gap { .. } | Stage::Fired { .. } => {}
        }
    }

    /// Пауза прошла: отправить разовый запрос со вспышкой поверх повторяющегося.
    fn submit_flash_still_request(&mut self, now: Instant) {
        let Some(FlashRun {
            shot,
            stage: Stage::Gap { planned, .. },
            ..
        }) = self.flash.run.as_ref().cloned()
        else {
            return;
        };
        let Some(applied) = self.session.as_ref().and_then(|session| session.repeating) else {
            self.fail_flash_stream_ended();
            return;
        };
        let controls = RequestControls {
            exposure: ExposurePlan::Sensor {
                exposure_time: planned.exposure_time,
                iso: planned.iso,
            },
            focus: applied.controls.focus,
            flash: FlashControl::Single,
        };
        match self.backend.submit_flash_still(applied.intent, &controls, shot) {
            Ok(()) => {
                let timeout = Duration::from_millis(self.config.three_a.flash.result_timeout_ms);
                if let Some(run) = self.flash.run.as_mut() {
                    run.stage = Stage::Fired {
                        planned,
                        deadline: now + timeout,
                    };
                }
            }
            Err(failure) => {
                tracing::warn!(target: LOG_TARGET, %failure, "разовый запрос со вспышкой не принят бэкендом");
                // Как у серии: сначала итог снимка, потом, если беда общая, восстановление сессии.
                if matches!(failure, RequestFailure::Backend(_)) {
                    self.fail_flash_keeping_stream(FlashStillFailure::SubmitFailed, now);
                } else {
                    self.finish_flash_run(|run| FlashStillReport::Failed {
                        shot: run.shot,
                        failure: FlashStillFailure::SubmitFailed,
                    });
                    let _ = self.fail_from_request_failure(failure, now);
                }
            }
        }
    }

    /// Результат кадра с меткой запроса вспышки. Чужой номер, поздний результат и повтор игнорируются.
    pub(in crate::engine) fn collect_flash_result(
        &mut self,
        shot: FlashShotId,
        result: &CaptureResultMeta,
        now: Instant,
    ) {
        let Some(FlashRun {
            shot: running,
            stage: Stage::Fired { planned, .. },
            accepted_at,
            ..
        }) = self.flash.run.as_ref().cloned()
        else {
            return;
        };
        if running != shot {
            return;
        }
        // `FIRED` не доказательство света (рендер решает по пикселям), но кадры без него считаем: так
        // видно, как часто HAL вообще не подтверждает вспышку.
        if result.three_a.flash.state != Some(FlashState::Fired) {
            self.flash.counters.results_not_fired += 1;
        }
        self.finish_flash_run(|_| FlashStillReport::Fired {
            shot,
            frame: FlashFrameResult {
                frame_number: result.frame_number,
                sensor_timestamp: result.sensor_timestamp,
                flash: result.three_a.flash,
            },
            plan: planned.plan,
            exposure_time: planned.exposure_time,
            iso: planned.iso,
            elapsed: now.saturating_duration_since(accepted_at),
        });
        self.resume_after_flash(now);
    }
}
