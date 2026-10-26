//! Сценарий `render-lifecycle`: показ переживает много сворачиваний и блокировок экрана.
//!
//! Скрипт (`scripts/smoke_driver.py`) отвечает на стадии действиями на телефоне, Rust ведёт счёт:
//! 1. `presenting_home {cycle}` — показ идёт, скрипт жмёт HOME;
//! 2. поток рендера отпустил surface (квитанция подтверждена) → `backgrounded_home {cycle}` — скрипт
//!    выжидает паузу и возвращает приложение; ждём, пока показ возобновится на новой surface;
//! 3. то же для экрана: `presenting_screen` (скрипт гасит экран) и `backgrounded_screen` (включает).
//!
//! Живые GPU-объекты меряем ТОЛЬКО В ПОКОЕ: когда поток рендера подтвердил освобождение surface и
//! сообщает «surface нет». Пока показ идёт, число объектов шумит (пул fence'ов показа, состояние
//! кадров в полёте: на телефоне 20/21 между замерами), и «равно при показе» недетерминировано.
//! Базовая перепись — снимок в покое после ПЕРВОГО освобождения (до первого attach статус пуст:
//! поток ещё не опрашивал бэкенд). Каждый следующий цикл сравнивается с ней ЦЕЛИКОМ, по видам
//! (`Fence`, `Image`, …), а не по сумме: утечка одного вида не спрячется за освобождением другого.
//! Счёт при показе пишется в отчёт справочно, вердикта по нему нет. Ограничение: разовая утечка
//! внутри самого первого цикла входит в базу и не видна; рост по циклам виден.
//!
//! Итог — [`evaluate`]: каждый цикл возобновил показ, подтверждений освобождения ровно столько, сколько
//! циклов, нет таймаутов освобождения в оболочке, перепись в покое не меняется, validation чиста,
//! неисправностей нет, Activity не пересоздавалась.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use render_loop::thread::{RenderState, RenderStatus};
use serde::Serialize;
use shell_contract::surface::SurfaceGeneration;

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields};
use super::render_metrics::{ValidationReportPart, Verdict, median, millis};
use crate::gpu_services::{RenderObservation, ValidationState};

/// Чем «закрываем» приложение в цикле.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleKind {
    /// Кнопка HOME и возврат.
    Home,
    /// Экран гаснет и включается.
    Screen,
}

impl CycleKind {
    fn presenting_stage(self) -> &'static str {
        match self {
            CycleKind::Home => "presenting_home",
            CycleKind::Screen => "presenting_screen",
        }
    }

    fn backgrounded_stage(self) -> &'static str {
        match self {
            CycleKind::Home => "backgrounded_home",
            CycleKind::Screen => "backgrounded_screen",
        }
    }
}

/// Что известно об одном цикле.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CycleRecord {
    pub cycle: u32,
    pub kind: CycleKind,
    /// Задержка освобождения surface в потоке рендера (от попадания запроса в ящик до подтверждения).
    pub release_latency_ms: Option<u64>,
    /// Живых GPU-объектов при показе перед закрытием приложения. Справочно: шумит от кадра к
    /// кадру, вердикта по нему нет.
    pub presenting_live_objects: Option<u64>,
    /// Перепись живых GPU-объектов по видам В ПОКОЕ (после подтверждённого освобождения surface;
    /// `None` — до покоя цикл не дошёл). Именно её сравнивает вердикт.
    pub idle_live_objects: Option<LiveObjectCensus>,
}

/// Живые GPU-объекты по видам (виды с нулём не перечисляются).
pub type LiveObjectCensus = BTreeMap<String, u64>;

/// Собранные факты для вердикта.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LifecycleEvidence {
    pub planned_cycles: u32,
    /// Перепись в покое после первого освобождения: с ней сравниваются все циклы.
    pub idle_baseline_live_objects: Option<LiveObjectCensus>,
    pub cycles: Vec<CycleRecord>,
    /// Сколько квитанций подтвердил поток рендера с начала сценария.
    pub releases_confirmed: u64,
    /// `surface_release_timeouts` оболочки: оболочка не дождалась подтверждения.
    pub surface_release_timeouts: u64,
    pub activity_recreated: bool,
    pub validation: ValidationReportPart,
    pub fault: Option<String>,
    pub backend_errors: u64,
    pub detach_failures: u64,
}

#[derive(Debug, Serialize)]
struct LatencySummary {
    max_ms: Option<f64>,
    median_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
struct LifecycleReport<'evidence> {
    #[serde(flatten)]
    evidence: &'evidence LifecycleEvidence,
    release_latency: LatencySummary,
}

/// Судит собранные факты.
pub fn evaluate(evidence: &LifecycleEvidence) -> Verdict {
    let mut violations = Vec::new();
    if let Some(fault) = &evidence.fault {
        violations.push(format!("поток рендера неисправен: {fault}"));
    }
    let completed = u32::try_from(evidence.cycles.len()).unwrap_or(u32::MAX);
    if completed != evidence.planned_cycles {
        violations.push(format!(
            "выполнено циклов {completed} из {} запланированных",
            evidence.planned_cycles
        ));
    }
    if evidence.releases_confirmed != u64::from(evidence.planned_cycles) {
        violations.push(format!(
            "поток рендера подтвердил освобождений: {} (ожидалось {})",
            evidence.releases_confirmed, evidence.planned_cycles
        ));
    }
    if evidence.surface_release_timeouts != 0 {
        violations.push(format!(
            "таймауты освобождения surface в оболочке: {} (ожидалось 0)",
            evidence.surface_release_timeouts
        ));
    }
    if let Some(baseline) = &evidence.idle_baseline_live_objects {
        for record in &evidence.cycles {
            match &record.idle_live_objects {
                Some(census) => {
                    if let Some(difference) = describe_census_difference(baseline, census) {
                        violations.push(format!("цикл {}: в покое {difference}", record.cycle));
                    }
                }
                None => violations.push(format!("цикл {}: живые GPU-объекты в покое не замерены", record.cycle)),
            }
        }
    } else {
        violations.push("базовая перепись GPU-объектов в покое не снята: первое освобождение не случилось".to_owned());
    }
    if evidence.activity_recreated {
        violations.push("Activity пересоздавалась (onCreate повторился)".to_owned());
    }
    violations.extend(evidence.validation.violation());
    if evidence.backend_errors != 0 {
        violations.push(format!("ошибки бэкенда рендера: {}", evidence.backend_errors));
    }
    if evidence.detach_failures != 0 {
        violations.push(format!(
            "отсоединение цепочки показа завершилось ошибкой {} раз",
            evidence.detach_failures
        ));
    }

    let latencies: Vec<Duration> = evidence
        .cycles
        .iter()
        .filter_map(|record| record.release_latency_ms.map(Duration::from_millis))
        .collect();
    let report = LifecycleReport {
        evidence,
        release_latency: LatencySummary {
            max_ms: latencies.iter().max().copied().map(millis),
            median_ms: median(&latencies).map(millis),
        },
    };
    Verdict::new(report, violations)
}

/// Словами описывает расхождение переписей: какие виды изменились, было и стало. `None` — одинаковы.
fn describe_census_difference(baseline: &LiveObjectCensus, census: &LiveObjectCensus) -> Option<String> {
    let kinds: std::collections::BTreeSet<&String> = baseline.keys().chain(census.keys()).collect();
    let changes: Vec<String> = kinds
        .into_iter()
        .filter_map(|kind| {
            let was = baseline.get(kind).copied().unwrap_or(0);
            let now = census.get(kind).copied().unwrap_or(0);
            (was != now).then(|| format!("{kind}: было {was}, стало {now}"))
        })
        .collect();
    (!changes.is_empty()).then(|| format!("живые GPU-объекты изменились ({})", changes.join("; ")))
}

/// Базовые значения на первом показе.
#[derive(Debug, Clone, Copy)]
struct Baseline {
    releases_confirmed: u64,
    activity_attachments: u64,
}

/// Где сценарий.
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Ждём показа на новой surface (поколение отличается от `previous_generation`).
    AwaitingPresenting {
        since: Instant,
        previous_generation: Option<SurfaceGeneration>,
    },
    /// Показ возобновился, ждём `render_settle_ms` перед замером.
    Settling { presenting_since: Instant },
    /// Скрипт закрыл приложение, ждём подтверждения освобождения surface.
    AwaitingRelease {
        since: Instant,
        releases_before: u64,
        generation_before: Option<SurfaceGeneration>,
        kind: CycleKind,
    },
}

pub struct RenderLifecycleScenario {
    phase: Option<Phase>,
    baseline: Option<Baseline>,
    /// Перепись в покое после первого освобождения.
    idle_baseline: Option<LiveObjectCensus>,
    cycles: Vec<CycleRecord>,
    /// Счётчик таймаутов освобождения оболочки в момент старта сценария. Счётчик оболочки
    /// считается от запуска процесса, поэтому судим по приросту: таймаут, случившийся до
    /// сценария (другой смок, ожидание диалога), ему не принадлежит.
    release_timeouts_at_start: u64,
}

impl RenderLifecycleScenario {
    pub fn new() -> Self {
        Self {
            phase: None,
            baseline: None,
            idle_baseline: None,
            cycles: Vec::new(),
            release_timeouts_at_start: 0,
        }
    }

    fn planned_cycles(context: &ScenarioContext<'_>) -> u32 {
        context.config.render_lifecycle_cycles + context.config.render_lifecycle_screen_cycles
    }

    /// Вид следующего цикла по его номеру с нуля: сначала HOME-циклы, затем экранные.
    fn kind_of_cycle(context: &ScenarioContext<'_>, index: u32) -> CycleKind {
        if index < context.config.render_lifecycle_cycles {
            CycleKind::Home
        } else {
            CycleKind::Screen
        }
    }

    fn advance(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if self.phase.is_none() {
            self.release_timeouts_at_start = context.services.diagnostics.counters().surface_release_timeouts;
        }
        let phase = *self.phase.get_or_insert(Phase::AwaitingPresenting {
            since: context.now,
            previous_generation: None,
        });
        let (status, validation) = match context.gpu.render.observe() {
            RenderObservation::Unavailable { reason } => {
                return ScenarioStep::Finished(failed(format!("рендер недоступен: {reason}")));
            }
            RenderObservation::Live { status, validation } => (*status, validation),
        };
        if status.fault.is_some() {
            return self.finish(context, &status, validation, Vec::new());
        }
        match phase {
            Phase::AwaitingPresenting {
                since,
                previous_generation,
            } => self.await_presenting(context, &status, validation, since, previous_generation),
            Phase::Settling { presenting_since } => self.settle(context, &status, validation, presenting_since),
            Phase::AwaitingRelease {
                since,
                releases_before,
                generation_before,
                kind,
            } => self.await_release(
                context,
                &status,
                validation,
                ReleaseWait {
                    since,
                    releases_before,
                    generation_before,
                    kind,
                },
            ),
        }
    }

    fn stage_timeout(context: &ScenarioContext<'_>) -> Duration {
        Duration::from_millis(context.config.render_stage_timeout_ms)
    }

    fn await_presenting(
        &mut self,
        context: &mut ScenarioContext<'_>,
        status: &RenderStatus,
        validation: ValidationState,
        since: Instant,
        previous_generation: Option<SurfaceGeneration>,
    ) -> ScenarioStep {
        let presenting_again = status.state == RenderState::Presenting
            && status.frames_presented > 0
            && status.generation.is_some()
            && status.generation != previous_generation;
        if presenting_again {
            self.phase = Some(Phase::Settling {
                presenting_since: context.now,
            });
            return ScenarioStep::Continue;
        }
        if context.now.saturating_duration_since(since) >= Self::stage_timeout(context) {
            let violation = format!(
                "после {} циклов показ не возобновился за {} мс (состояние {:?}, surface {:?})",
                self.cycles.len(),
                Self::stage_timeout(context).as_millis(),
                status.state,
                status.generation
            );
            return self.finish(context, status, validation, vec![violation]);
        }
        ScenarioStep::Continue
    }

    fn settle(
        &mut self,
        context: &mut ScenarioContext<'_>,
        status: &RenderStatus,
        validation: ValidationState,
        presenting_since: Instant,
    ) -> ScenarioStep {
        if status.state != RenderState::Presenting {
            let violation = format!("показ прервался до замера объектов (состояние {:?})", status.state);
            return self.finish(context, status, validation, vec![violation]);
        }
        let settle = Duration::from_millis(context.config.render_settle_ms);
        if context.now.saturating_duration_since(presenting_since) < settle {
            return ScenarioStep::Continue;
        }
        let live_objects = status.backend.live_objects;
        if self.baseline.is_none() {
            self.baseline = Some(Baseline {
                releases_confirmed: status.releases_confirmed,
                activity_attachments: context.services.diagnostics.counters().activity_attachments,
            });
        }
        let completed = u32::try_from(self.cycles.len()).unwrap_or(u32::MAX);
        if completed >= Self::planned_cycles(context) {
            return self.finish(context, status, validation, Vec::new());
        }
        // Показ стабилен: просим скрипт закрыть приложение этим видом цикла.
        let kind = Self::kind_of_cycle(context, completed);
        self.cycles.push(CycleRecord {
            cycle: completed + 1,
            kind,
            release_latency_ms: None,
            presenting_live_objects: Some(live_objects),
            idle_live_objects: None,
        });
        context.emit_stage(
            kind.presenting_stage(),
            stage_fields([
                ("cycle", (completed + 1).into()),
                ("of", Self::planned_cycles(context).into()),
                ("live_objects", live_objects.into()),
            ]),
        );
        self.phase = Some(Phase::AwaitingRelease {
            since: context.now,
            releases_before: status.releases_confirmed,
            generation_before: status.generation,
            kind,
        });
        ScenarioStep::Continue
    }

    fn await_release(
        &mut self,
        context: &mut ScenarioContext<'_>,
        status: &RenderStatus,
        validation: ValidationState,
        wait: ReleaseWait,
    ) -> ScenarioStep {
        // «Покой» — подтверждение получено И поток рендера сообщает «surface нет»: тогда статус
        // содержит снимок, снятый сразу после detach (см. `detach_surface` в render-loop).
        let released = status.releases_confirmed > wait.releases_before
            && status.state == RenderState::WaitingForSurface
            && status.generation.is_none();
        if released {
            let idle_census = status.backend.live_objects_by_kind.clone();
            if self.idle_baseline.is_none() {
                self.idle_baseline = Some(idle_census.clone());
            }
            let cycle = u32::try_from(self.cycles.len()).unwrap_or(u32::MAX);
            if let Some(last) = self.cycles.last_mut() {
                last.idle_live_objects = Some(idle_census);
                last.release_latency_ms = status
                    .last_release_latency
                    .map(|latency| u64::try_from(latency.as_millis()).unwrap_or(u64::MAX));
            }
            context.emit_stage(wait.kind.backgrounded_stage(), stage_fields([("cycle", cycle.into())]));
            self.phase = Some(Phase::AwaitingPresenting {
                since: context.now,
                previous_generation: wait.generation_before,
            });
            return ScenarioStep::Continue;
        }
        if context.now.saturating_duration_since(wait.since) >= Self::stage_timeout(context) {
            let violation = format!(
                "цикл {}: поток рендера не освободил surface за {} мс (состояние {:?})",
                self.cycles.len(),
                Self::stage_timeout(context).as_millis(),
                status.state
            );
            return self.finish(context, status, validation, vec![violation]);
        }
        ScenarioStep::Continue
    }

    /// Собирает факты и выносит вердикт; `extra` — причины досрочного завершения.
    fn finish(
        &self,
        context: &ScenarioContext<'_>,
        status: &RenderStatus,
        validation: ValidationState,
        extra: Vec<String>,
    ) -> ScenarioStep {
        let counters = context.services.diagnostics.counters();
        let baseline = self.baseline;
        let evidence = LifecycleEvidence {
            planned_cycles: Self::planned_cycles(context),
            idle_baseline_live_objects: self.idle_baseline.clone(),
            cycles: self.cycles.clone(),
            releases_confirmed: status
                .releases_confirmed
                .saturating_sub(baseline.map_or(0, |base| base.releases_confirmed)),
            surface_release_timeouts: counters
                .surface_release_timeouts
                .saturating_sub(self.release_timeouts_at_start),
            activity_recreated: baseline.is_some_and(|base| base.activity_attachments != counters.activity_attachments),
            validation: ValidationReportPart::of(status, validation),
            fault: status
                .fault
                .as_ref()
                .map(|fault| format!("{:?}: {}", fault.kind, fault.message)),
            backend_errors: status.backend_errors,
            detach_failures: status.backend.detach_failures,
        };
        let mut verdict = evaluate(&evidence);
        verdict.violations.splice(0..0, extra);
        ScenarioStep::Finished(verdict.into_outcome())
    }
}

/// Что ждёт фаза `AwaitingRelease` (чтобы не таскать четыре параметра по сигнатурам).
#[derive(Debug, Clone, Copy)]
struct ReleaseWait {
    since: Instant,
    releases_before: u64,
    generation_before: Option<SurfaceGeneration>,
    kind: CycleKind,
}

impl Default for RenderLifecycleScenario {
    fn default() -> Self {
        Self::new()
    }
}

impl ObservingScenario for RenderLifecycleScenario {
    fn name(&self) -> &'static str {
        "render-lifecycle"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }

    fn on_event(&mut self, _event: ObservedEvent, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }
}

#[cfg(test)]
#[path = "render_lifecycle_tests.rs"]
mod tests;
