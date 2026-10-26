//! Сценарий `lifecycle`: свернуть и развернуть приложение без утечек и с понятным порядком событий.
//!
//! Ход (скрипт — `scripts/smoke_driver.py`):
//! 1. приложение на экране, surface есть и её размер известен → стадия `ready` (скрипт жмёт HOME);
//! 2. пришли `paused` и `surface_destroyed` → стадия `backgrounded` (скрипт возвращает приложение);
//! 3. снова на экране, с surface и размером → итог: события после `ready` по порядку, живые глобальные
//!    ссылки JNI до и после, таймауты освобождения surface, пересоздавалась ли Activity.

use serde::Serialize;
use shell_contract::event::ShellEventKind;

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, passed, stage_fields};

/// Какие события попадают в отчёт: жизненный цикл, surface, разрешение и отказы платформы.
/// Касания, тепло и ориентация шумят и порядок не проверяют — в отчёт не идут.
fn is_reported(kind: ShellEventKind) -> bool {
    !matches!(
        kind,
        ShellEventKind::Touch
            | ShellEventKind::TouchGap
            | ShellEventKind::ThermalStatus
            | ShellEventKind::DeviceOrientation
    )
}

/// Где сейчас сценарий: до `ready`, на переднем плане после `ready`, в фоне после `backgrounded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    BeforeReady,
    InForeground,
    InBackground,
}

/// Снимок счётчиков в момент `ready`.
#[derive(Debug, Clone, Copy)]
struct ReadyBaseline {
    live_global_refs: u64,
    activity_attachments: u64,
}

#[derive(Debug, Serialize)]
struct LifecycleReport {
    events: Vec<ShellEventKind>,
    live_global_refs_at_ready: u64,
    live_global_refs_at_end: u64,
    surface_release_timeouts: u64,
    activity_recreated: bool,
    touch_samples_dropped: u64,
    critical_events_lost: u64,
}

pub struct LifecycleScenario {
    phase: Phase,
    baseline: Option<ReadyBaseline>,
    events: Vec<ShellEventKind>,
    saw_paused: bool,
    saw_surface_destroyed: bool,
}

impl LifecycleScenario {
    pub fn new() -> Self {
        Self {
            phase: Phase::BeforeReady,
            baseline: None,
            events: Vec::new(),
            saw_paused: false,
            saw_surface_destroyed: false,
        }
    }

    fn become_ready(&mut self, context: &mut ScenarioContext<'_>) {
        let counters = context.services.diagnostics.counters();
        self.baseline = Some(ReadyBaseline {
            live_global_refs: counters.live_global_refs,
            activity_attachments: counters.activity_attachments,
        });
        self.phase = Phase::InForeground;
        context.emit_stage(
            "ready",
            stage_fields([("live_global_refs", counters.live_global_refs.into())]),
        );
    }

    fn finish(&self, context: &ScenarioContext<'_>) -> ScenarioStep {
        let counters = context.services.diagnostics.counters();
        let baseline = self.baseline.unwrap_or(ReadyBaseline {
            live_global_refs: 0,
            activity_attachments: 0,
        });
        ScenarioStep::Finished(passed(LifecycleReport {
            events: self.events.clone(),
            live_global_refs_at_ready: baseline.live_global_refs,
            live_global_refs_at_end: counters.live_global_refs,
            surface_release_timeouts: counters.surface_release_timeouts,
            activity_recreated: counters.activity_attachments != baseline.activity_attachments,
            touch_samples_dropped: counters.touch_samples_dropped,
            critical_events_lost: counters.critical_events_lost,
        }))
    }
}

impl Default for LifecycleScenario {
    fn default() -> Self {
        Self::new()
    }
}

impl ObservingScenario for LifecycleScenario {
    fn name(&self) -> &'static str {
        "lifecycle"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if context.display.is_ready_to_draw() {
            self.become_ready(context);
        }
        ScenarioStep::Continue
    }

    fn on_event(&mut self, event: ObservedEvent, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        match self.phase {
            Phase::BeforeReady => {
                if context.display.is_ready_to_draw() {
                    self.become_ready(context);
                }
                ScenarioStep::Continue
            }
            Phase::InForeground => {
                self.record(event.kind);
                self.saw_paused |= event.kind == ShellEventKind::Paused;
                self.saw_surface_destroyed |= event.kind == ShellEventKind::SurfaceDestroyed;
                if self.saw_paused && self.saw_surface_destroyed {
                    self.phase = Phase::InBackground;
                    context.emit_stage("backgrounded", serde_json::Map::new());
                }
                ScenarioStep::Continue
            }
            Phase::InBackground => {
                self.record(event.kind);
                if context.display.is_ready_to_draw() {
                    return self.finish(context);
                }
                ScenarioStep::Continue
            }
        }
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        // Ждёт только событий; таймаут ожидания — у скрипта.
        ScenarioStep::Continue
    }
}

impl LifecycleScenario {
    fn record(&mut self, kind: ShellEventKind) {
        if is_reported(kind) {
            self.events.push(kind);
        }
    }
}
