//! Ход записи сценария `ae-trace`: расписание -> события (профиль, подсказки на экране) -> отсчёты ряда.
//!
//! Модуль владеет состоянием записи ([`Tracing`]) и не знает ни о запуске видоискателя, ни о том, как
//! прогон заканчивается: это дело `ae_trace.rs`. Снаружи — одно намерение: «шаг записи в момент `now` с этими
//! показаниями» ([`Tracing::advance`]).
//!
//! Что нельзя нарушить:
//! * событие расписания исполняется ровно один раз (`Timeline::events_due`);
//! * профиль 3A меняется только на начале фазы, отказ очереди — провал прогона, не тишина;
//! * каждая подсказка владельцу записана в журнал вместе с исходом платформы.

use std::time::Instant;

use camera_core::CameraHandle;
use camera_core::three_a_status::ThreeAStatus;
use render_loop::viewfinder::AutoExposureDiagnostics;
use shell_contract::operator_prompt::OperatorPromptService;

use super::ae_trace_plan::{PlanEvent, ScheduledEvent, Timeline};
use super::ae_trace_prompts::{moment_of, prompt_for};
use super::ae_trace_series::{PushOutcome, SampleTaker, SeriesBuffer};
use super::engine_watch::{RunFailure, rejected_failure};
use super::operator_prompt_log::PromptLog;
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Что запись сообщает после шага.
#[derive(Debug, PartialEq, Eq)]
pub enum Advance {
    /// Идём дальше.
    Continue,
    /// Расписание исчерпано: можно разбирать ряд.
    ScheduleFinished,
}

/// Состояние записи.
pub struct Tracing {
    started_at: Instant,
    timeline: Timeline,
    /// Сколько событий расписания уже исполнено.
    fired: usize,
    last_sample_ms: Option<u64>,
    taker: SampleTaker,
    series: SeriesBuffer,
    prompts: PromptLog,
    /// Тиков без диагностики AE после начала записи.
    ticks_without_ae: u64,
}

/// Что запись отдаёт после конца (в любом исходе).
pub struct TracingOutcome {
    pub timeline: Timeline,
    pub series: Vec<super::ae_trace_series::TraceSample>,
    pub dropped_samples: u64,
    pub prompts: PromptLog,
    pub ticks_without_ae: u64,
}

impl Tracing {
    /// Начало записи: момент `now` — ноль расписания.
    pub fn begin(config: &AeTraceScenarioConfig, now: Instant) -> Self {
        Self {
            started_at: now,
            timeline: Timeline::from_config(config),
            fired: 0,
            last_sample_ms: None,
            taker: SampleTaker::default(),
            series: SeriesBuffer::with_capacity(config.series_capacity),
            prompts: PromptLog::default(),
            ticks_without_ae: 0,
        }
    }

    /// Тик без диагностики AE: запись продолжается, пропуск считается (его видно в отчёте).
    pub fn note_missing_diagnostics(&mut self, now: Instant) -> Advance {
        self.ticks_without_ae += 1;
        self.finished_at(now)
    }

    /// Шаг записи: исполняет наступившие события расписания, пишет отсчёт, если пора.
    /// Провал — отказ очереди движка при смене профиля.
    pub fn advance(
        &mut self,
        config: &AeTraceScenarioConfig,
        now: Instant,
        services: RecordingServices<'_>,
        readings: Readings<'_>,
    ) -> Result<Advance, RunFailure> {
        let elapsed_ms = self.elapsed_ms(now);
        let due: Vec<ScheduledEvent> = self.timeline.events_due(self.fired, elapsed_ms).to_vec();
        self.fired += due.len();
        for scheduled in due {
            self.execute(scheduled, &services)?;
        }
        if self.sample_is_due(config, elapsed_ms) {
            let sample = self.taker.take(
                elapsed_ms,
                readings.diagnostics,
                readings.three_a.last_result.as_ref(),
                readings.three_a.profile,
            );
            // Переполнение учитывает сам ряд (`dropped`); вердикт его покажет.
            let _outcome: PushOutcome = self.series.push(sample);
            self.last_sample_ms = Some(elapsed_ms);
        }
        Ok(self.finished_at(now))
    }

    /// Забирает результаты записи.
    pub fn finish(self) -> TracingOutcome {
        TracingOutcome {
            timeline: self.timeline,
            dropped_samples: self.series.dropped(),
            series: self.series.samples().to_vec(),
            prompts: self.prompts,
            ticks_without_ae: self.ticks_without_ae,
        }
    }

    fn elapsed_ms(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.started_at).as_millis()).unwrap_or(u64::MAX)
    }

    fn finished_at(&self, now: Instant) -> Advance {
        if self.timeline.is_finished(self.elapsed_ms(now)) {
            Advance::ScheduleFinished
        } else {
            Advance::Continue
        }
    }

    fn sample_is_due(&self, config: &AeTraceScenarioConfig, elapsed_ms: u64) -> bool {
        self.last_sample_ms
            .is_none_or(|last| elapsed_ms.saturating_sub(last) >= config.sample_interval_ms)
    }

    /// Исполняет одно событие: смена профиля (только на начале фазы) и подсказка владельцу на экране.
    /// Подсказка — состояние: она висит, пока её не заменит следующее событие расписания или не снимет
    /// маршрутизатор при конце сценария.
    fn execute(&mut self, scheduled: ScheduledEvent, services: &RecordingServices<'_>) -> Result<(), RunFailure> {
        if let PlanEvent::PhaseStart(phase) = scheduled.event {
            services
                .handle
                .set_three_a_profile(phase.profile())
                .map_err(|rejected| rejected_failure("set_three_a_profile", rejected))?;
        }
        let prompt = prompt_for(scheduled.event, &self.timeline);
        self.prompts
            .show(services.prompts, &moment_of(scheduled.event), &prompt);
        Ok(())
    }
}

/// Кому запись отдаёт команды: движок камеры и экран подсказок владельцу.
pub struct RecordingServices<'call> {
    pub handle: &'call CameraHandle,
    pub prompts: &'call dyn OperatorPromptService,
}

/// Показания одного тика.
pub struct Readings<'call> {
    pub diagnostics: &'call AutoExposureDiagnostics,
    pub three_a: &'call ThreeAStatus,
}

#[cfg(test)]
#[path = "ae_trace_run_tests.rs"]
mod tests;
