//! Порядок прогона стенда насыщения: работы по одной, пауза между ними, отмена, тепло, события.
//!
//! Раннер НЕ знает GPU: создание работы, прогон стенда и освобождение — за [`WorkloadExecutor`] (настоящий —
//! [`super::gpu_executor`], подставной — в тестах). Часы и паузы — за [`RunClock`]: тест проверяет паузу без
//! `sleep`. Тепло — за [`ThermalProbe`].
//!
//! # Инварианты
//! * работы идут строго по очереди; следующая создаётся только после возврата предыдущей, а исполнитель к этому
//!   моменту обязан её полностью освободить;
//! * ошибка одной работы не срывает остальные: в таблице строка с типизированной причиной;
//! * отмена посреди списка: прерванная работа возвращает частичный итог (`EndReason::Cancelled`), остальные
//!   помечены «не запускалась», итог отдаётся целиком (частичный лучше отсутствующего);
//! * паузы нет перед первой работой и после последней; пауза прерывается отменой.

use std::time::Duration;

use gpu_vulkan::saturation::CancelToken;

use super::catalog::CatalogEntry;
use super::digest_report::DigestOutcome;
use super::extras::WorkloadExtras;
use super::report::{
    NotRunReason, RowOutcome, RunSummary, SceneSummary, UiCountersSummary, WorkloadFailure, WorkloadRow,
};
use crate::power_measurement::ThermalSnapshot;

pub use super::report::Measurement;

/// Что вернул исполнитель по удавшейся работе.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutedWorkload {
    pub measurement: Measurement,
    pub scene: Option<SceneSummary>,
    pub ui: Option<UiCountersSummary>,
    /// Дайджест выхода работы (прочитан после окна замера).
    pub digest: DigestOutcome,
    /// Дополнительные факты (паритет, размер картинки прибора, счётчик пайплайнов).
    pub extras: WorkloadExtras,
}

/// Исполнитель одной работы: создать → прогнать стенд → освободить полностью.
pub trait WorkloadExecutor {
    /// Блокирующий вызов. К возврату работа ОБЯЗАНА быть освобождена (и при ошибке тоже). Отмену исполнитель
    /// отдаёт частичным итогом, а не ошибкой.
    fn execute(&mut self, entry: &CatalogEntry, cancel: &CancelToken) -> Result<ExecutedWorkload, WorkloadFailure>;
}

/// Как кончилась пауза.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseOutcome {
    Elapsed,
    /// Отмена пришла раньше конца паузы.
    Cancelled,
}

/// Часы прогона и паузы.
pub trait RunClock {
    /// Сколько прошло от начала прогона.
    fn elapsed(&self) -> Duration;
    /// Пауза, прерываемая отменой.
    fn pause(&self, duration: Duration, cancel: &CancelToken) -> PauseOutcome;
}

/// Источник теплового снимка (на телефоне — сервис оболочки).
pub trait ThermalProbe {
    fn snapshot(&self) -> ThermalSnapshot;
}

/// Событие прогона для наблюдателя (сценарий пишет по ним стадии).
#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    /// Работа начинается: оркестратор по этим стадиям совмещает журнал тепла с ПК по времени.
    WorkloadStarted {
        index: usize,
        total: usize,
        name: &'static str,
        offset: Duration,
        thermal: ThermalSnapshot,
    },
    /// Работа закончилась (любым исходом).
    WorkloadFinished {
        index: usize,
        total: usize,
        name: &'static str,
        offset: Duration,
        verdict_word: &'static str,
    },
}

/// Получатель событий прогона.
pub trait RunObserver {
    fn on_event(&mut self, event: RunEvent);
}

impl<F: FnMut(RunEvent)> RunObserver for F {
    fn on_event(&mut self, event: RunEvent) {
        self(event);
    }
}

/// Раннер: всё, что нужно для прогона списка, кроме самого списка.
pub struct BenchRunner<'env> {
    pub executor: &'env mut dyn WorkloadExecutor,
    pub clock: &'env dyn RunClock,
    pub thermal: &'env dyn ThermalProbe,
    pub observer: &'env mut dyn RunObserver,
    /// Пауза между работами.
    pub cooldown_between: Duration,
}

impl BenchRunner<'_> {
    /// Прогоняет `entries` по порядку; всегда возвращает итог на каждую работу списка.
    pub fn run(&mut self, entries: &[&'static CatalogEntry], cancel: &CancelToken) -> RunSummary {
        let total = entries.len();
        let mut rows: Vec<WorkloadRow> = Vec::with_capacity(total);
        for (position, entry) in entries.iter().enumerate() {
            let index = position + 1;
            if cancel.is_cancelled() || !self.pause_before(position, cancel) {
                rows.push(WorkloadRow::not_run(entry.name, index, total, NotRunReason::Cancelled));
                continue;
            }
            rows.push(self.run_one(entry, index, total, cancel));
        }
        RunSummary {
            rows,
            cancelled: cancel.is_cancelled(),
        }
    }

    /// Пауза между работами (перед первой её нет). `false` — пауза прервана отменой.
    fn pause_before(&self, position: usize, cancel: &CancelToken) -> bool {
        if position == 0 || self.cooldown_between.is_zero() {
            return true;
        }
        self.clock.pause(self.cooldown_between, cancel) == PauseOutcome::Elapsed
    }

    fn run_one(
        &mut self,
        entry: &'static CatalogEntry,
        index: usize,
        total: usize,
        cancel: &CancelToken,
    ) -> WorkloadRow {
        let started_at = self.clock.elapsed();
        let thermal_start = self.thermal.snapshot();
        self.observer.on_event(RunEvent::WorkloadStarted {
            index,
            total,
            name: entry.name,
            offset: started_at,
            thermal: thermal_start.clone(),
        });
        let executed = self.executor.execute(entry, cancel);
        let thermal_end = self.thermal.snapshot();
        let (outcome, scene, ui, digest, extras) = match executed {
            Ok(done) => {
                let outcome = if done.measurement.is_measured() {
                    RowOutcome::Measured {
                        measurement: done.measurement,
                    }
                } else {
                    RowOutcome::NoFramesCompleted {
                        measurement: done.measurement,
                    }
                };
                (outcome, done.scene, done.ui, Some(done.digest), done.extras)
            }
            Err(failure) => (
                RowOutcome::Failed { failure },
                None,
                None,
                None,
                WorkloadExtras::default(),
            ),
        };
        let row = WorkloadRow {
            name: entry.name,
            index,
            total,
            outcome,
            scene,
            ui,
            digest,
            extras,
            thermal_start: Some(thermal_start),
            thermal_end: Some(thermal_end),
            started_at_ms: Some(started_at.as_secs_f64() * 1_000.0),
        };
        self.observer.on_event(RunEvent::WorkloadFinished {
            index,
            total,
            name: entry.name,
            offset: self.clock.elapsed(),
            verdict_word: row.verdict_word(),
        });
        row
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
