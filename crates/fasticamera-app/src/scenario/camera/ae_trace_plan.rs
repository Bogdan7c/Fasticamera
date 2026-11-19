//! Расписание сценария `ae-trace`: какая фаза идёт, когда подсказка владельцу, когда конец (чистая логика).
//!
//! Расписание строится один раз из конфига и дальше только отвечает на вопросы «что наступило к моменту
//! `t`». Ни часов, ни вибромотора, ни камеры здесь нет: сценарий (`ae_trace.rs`) приносит время и сам
//! исполняет события. Так порядок фаз и места подсказок проверяются тестом без телефона.
//!
//! Время — миллисекунды от начала расписания (от первой диагностики нашего AE, не от старта сценария).

use camera_contract::three_a::ThreeAProfile;
use serde::Serialize;

use crate::ae_trace_scenario_config::AeTraceScenarioConfig;

/// Фаза прогона. Порядок фиксирован: `photo`, `video`, `static`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TracePhase {
    /// Профиль 3A «Фото», владелец переключает лампу по подсказкам.
    Photo,
    /// Профиль 3A «Видео», владелец переключает лампу по подсказкам.
    Video,
    /// Профиль «Фото», подсказок нет: владелец ничего не трогает, ищем «дыхание».
    Static,
}

impl TracePhase {
    /// Все фазы по порядку прогона.
    pub const ALL: [TracePhase; 3] = [TracePhase::Photo, TracePhase::Video, TracePhase::Static];

    /// Имя для отчёта (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            TracePhase::Photo => "photo",
            TracePhase::Video => "video",
            TracePhase::Static => "static",
        }
    }

    /// Профиль 3A, который движок обязан держать в этой фазе.
    pub fn profile(self) -> ThreeAProfile {
        match self {
            TracePhase::Photo | TracePhase::Static => ThreeAProfile::Photo,
            TracePhase::Video => ThreeAProfile::Video,
        }
    }

    /// Бывают ли в фазе подсказки (в `static` владелец ничего не трогает).
    pub fn has_cues(self) -> bool {
        !matches!(self, TracePhase::Static)
    }
}

/// Окно одной фазы на оси времени расписания.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseWindow {
    pub phase: TracePhase,
    /// Начало (включительно), мс.
    pub start_ms: u64,
    /// Конец (не включительно), мс.
    pub end_ms: u64,
    /// Моменты подсказок внутри фазы, мс, по возрастанию. Пусто — подсказок нет.
    pub cues_ms: Vec<u64>,
}

impl PhaseWindow {
    /// Конец отрезка после подсказки `index`: следующая подсказка или конец фазы.
    pub fn segment_end_ms(&self, index: usize) -> u64 {
        self.cues_ms.get(index + 1).copied().unwrap_or(self.end_ms)
    }
}

/// Что должно произойти в момент расписания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanEvent {
    /// Началась фаза: переключить профиль и дать отличимый сигнал.
    PhaseStart(TracePhase),
    /// Подсказка владельцу переключить лампу (`index` — номер внутри фазы с нуля).
    Cue { phase: TracePhase, index: usize },
}

/// Событие с моментом исполнения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledEvent {
    pub at_ms: u64,
    pub event: PlanEvent,
}

/// Расписание прогона целиком.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    phases: Vec<PhaseWindow>,
    events: Vec<ScheduledEvent>,
}

impl Timeline {
    /// Строит расписание из конфига. Конфиг обязан быть проверен (`AeTraceScenarioConfig::validate`).
    pub fn from_config(config: &AeTraceScenarioConfig) -> Self {
        let mut phases = Vec::new();
        let mut events = Vec::new();
        let mut start_ms = 0_u64;
        for phase in TracePhase::ALL {
            let length_ms = match phase {
                TracePhase::Photo => config.photo_phase_ms,
                TracePhase::Video => config.video_phase_ms,
                TracePhase::Static => config.static_phase_ms,
            };
            let end_ms = start_ms.saturating_add(length_ms);
            events.push(ScheduledEvent {
                at_ms: start_ms,
                event: PlanEvent::PhaseStart(phase),
            });
            let cues_ms = cue_moments(phase, start_ms, end_ms, config.cue_interval_ms);
            for (index, at_ms) in cues_ms.iter().enumerate() {
                events.push(ScheduledEvent {
                    at_ms: *at_ms,
                    event: PlanEvent::Cue { phase, index },
                });
            }
            phases.push(PhaseWindow {
                phase,
                start_ms,
                end_ms,
                cues_ms,
            });
            start_ms = end_ms;
        }
        Self { phases, events }
    }

    /// Окна фаз по порядку.
    pub fn phases(&self) -> &[PhaseWindow] {
        &self.phases
    }

    /// Конец расписания, мс.
    pub fn total_ms(&self) -> u64 {
        self.phases.last().map_or(0, |window| window.end_ms)
    }

    /// Расписание исчерпано к моменту `elapsed_ms`.
    pub fn is_finished(&self, elapsed_ms: u64) -> bool {
        elapsed_ms >= self.total_ms()
    }

    /// События, наступившие к `elapsed_ms` и ещё не выданные: `already_fired` — сколько уже выдано.
    /// Вызывающий прибавляет длину ответа к своему счётчику; событие не может выдаться дважды.
    pub fn events_due(&self, already_fired: usize, elapsed_ms: u64) -> &[ScheduledEvent] {
        let pending = self.events.get(already_fired..).unwrap_or(&[]);
        let due = pending.partition_point(|scheduled| scheduled.at_ms <= elapsed_ms);
        &pending[..due]
    }
}

/// Подсказки фазы: через период после начала, пока момент строго внутри фазы.
fn cue_moments(phase: TracePhase, start_ms: u64, end_ms: u64, interval_ms: u64) -> Vec<u64> {
    if !phase.has_cues() {
        return Vec::new();
    }
    (1_u64..)
        .map(|step| start_ms.saturating_add(step.saturating_mul(interval_ms)))
        .take_while(|at_ms| *at_ms < end_ms)
        .collect()
}

#[cfg(test)]
#[path = "ae_trace_plan_tests.rs"]
mod tests;
