//! Сценарий `camera-catalog`: дождаться каталога камер и выложить в отчёт всё, что телефон о них сказал.
//!
//! Сценарий ничего не открывает: каталог читается движком при старте. Он опрашивает снимок движка
//! по тикам маршрутизатора (ничего не блокируя) до прочитанного каталога, окончательной ошибки или
//! дедлайна `camera.scenarios.catalog_timeout_ms`. Вердикт — [`evaluate_catalog`](super::catalog_report::evaluate_catalog).
//! Сверку каталога с `dumpsys media.camera` делает скрипт после результата (`scripts/smoke_dumpsys.py`).

use std::time::Instant;

use camera_core::{CameraError, CameraState, CameraStatus};

use super::catalog_report::{CatalogDetails, CatalogReport, evaluate_catalog};
use super::detail::emit_details;
use super::progress::{Deadline, EventLog};
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, judged, stage_fields,
};

/// Чем кончилось ожидание каталога.
enum CatalogWait {
    /// Каталог прочитан.
    Ready,
    /// Движок в окончательной ошибке: каталога не будет.
    Fatal(String),
    /// Каталог ещё не прочитан, срок не вышел.
    Pending,
    /// Срок вышел, каталога нет.
    TimedOut,
}

fn catalog_wait(status: &CameraStatus, deadline: Deadline, now: Instant) -> CatalogWait {
    if let CameraState::Error {
        error: CameraError::Fatal { cause },
    } = &status.state
    {
        return CatalogWait::Fatal(format!("{cause:?}"));
    }
    if !status.catalog.cameras().is_empty() {
        return CatalogWait::Ready;
    }
    if deadline.is_expired(now) {
        CatalogWait::TimedOut
    } else {
        CatalogWait::Pending
    }
}

/// Прогон сценария: срок и журнал событий движка.
struct Run {
    deadline: Deadline,
    events: EventLog,
}

#[derive(Default)]
pub struct CameraCatalogScenario {
    run: Option<Run>,
}

impl CameraCatalogScenario {
    pub fn new() -> Self {
        Self::default()
    }

    /// Итог по снимку: отчёт всегда содержит то, что движок успел узнать, даже при провале.
    /// Подробности уходят строками стадий ДО результата (строка результата ограничена 4000 байтами).
    fn finish(
        &self,
        context: &mut ScenarioContext<'_>,
        running: &RunningCamera,
        status: &CameraStatus,
        wait: &CatalogWait,
    ) -> ScenarioStep {
        let Some(run) = &self.run else {
            return ScenarioStep::Continue;
        };
        let backend = running.diagnostics_snapshot();
        let details = CatalogDetails::build(&status.catalog, &backend);
        emit_details(context, "detail_cameras", details.cameras);
        emit_details(context, "detail_backend", details.backend);
        let report = CatalogReport::build(
            status.state.name(),
            &status.catalog,
            &backend,
            status.counters,
            run.events.notes().len(),
        );
        let mut violations = evaluate_catalog(&status.catalog);
        match wait {
            CatalogWait::Fatal(cause) => violations.insert(0, format!("движок камеры в окончательной ошибке: {cause}")),
            CatalogWait::TimedOut => violations.insert(0, "каталог не прочитан за отведённое время".to_owned()),
            CatalogWait::Ready | CatalogWait::Pending => {}
        }
        ScenarioStep::Finished(judged(report, violations))
    }
}

impl ObservingScenario for CameraCatalogScenario {
    fn name(&self) -> &'static str {
        "camera-catalog"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if let Err(unavailable) = context.camera.running() {
            return ScenarioStep::Finished(failed(unavailable.to_string()));
        }
        let timeout_ms = context.camera.scenarios().catalog_timeout_ms;
        self.run = Some(Run {
            deadline: Deadline::after_millis(context.now, timeout_ms),
            events: EventLog::started_at(context.now),
        });
        context.emit_stage("started", stage_fields([("timeout_ms", timeout_ms.into())]));
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Ok(running) = context.camera.running() else {
            return ScenarioStep::Continue;
        };
        let Some(run) = &mut self.run else {
            return ScenarioStep::Continue;
        };
        let events = running.handle().drain_events(usize::MAX);
        run.events.absorb(context.now, &events);
        let status = running.handle().status();
        let wait = catalog_wait(&status, run.deadline, context.now);
        match wait {
            CatalogWait::Pending => ScenarioStep::Continue,
            CatalogWait::Ready | CatalogWait::Fatal(_) | CatalogWait::TimedOut => {
                self.finish(context, running, &status, &wait)
            }
        }
    }
}
