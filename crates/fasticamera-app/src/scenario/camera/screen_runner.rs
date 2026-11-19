//! Исполнитель плана `screen`: по одному шагу просит жест у скрипта и ждёт его следствий на экране и в камере.
//!
//! Конечный автомат без ввода-вывода: на каждом тике получает время и снимок ([`ScreenProbe`]) и отвечает, что
//! сделать сценарию ([`RunnerTick`]). Поэтому все правила (срок шага, выдержка, перерост счётчика, отказ HAL)
//! проверяются тестами без маршрутизатора.
//!
//! # Жизнь шага
//! ```text
//!  BeginStep ──▶ Waiting(срок) ──ожидание достигнуто──▶ Settling(выдержка) ──▶ проверка «ничего лишнего» ──▶ следующий
//!                   │                                        │
//!                   └─ срок вышел / счётчик перерос ─────────┴─▶ Failed("ожидали X, увидели Y")
//! ```
//! Стадию шага сценарий выдаёт РОВНО один раз (`BeginStep`): драйвер выполняет шаги стадии при каждой её выдаче.
//!
//! # Сквозные отказы
//! Независимо от шага провал прогона — это (1) намерение камере, которое очередь движка не приняла
//! (`camera_intents_lost`), и (2) триггер, который HAL отверг (`focus_trigger_failures`): тап «дошёл до движка, но
//! не до HAL». Они проверяются на каждом тике по приращению от начала плана.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::screen_expect::Expectation;
use super::screen_probe::{ProbeDelta, ScreenProbe};
use super::screen_steps::PlannedStep;

/// Сроки шага.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTiming {
    /// Сколько ждать достижения ожидания после стадии.
    pub timeout: Duration,
    /// Выдержка после достижения: за неё не должно случиться ничего лишнего.
    pub settle: Duration,
}

/// Запись о шаге для отчёта.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepRecord {
    pub name: &'static str,
    pub ok: bool,
    /// Вывод о состоянии экрана косвенный (см. `screen_steps.rs`).
    pub inferred: bool,
    /// Что увидели: приращения счётчиков и значения. Для провала — ещё и «ожидали X, увидели Y».
    pub observed: String,
}

/// Что сценарию делать после тика.
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerTick {
    /// Ничего нового: ждём.
    Waiting,
    /// Начался шаг: выдать стадию `screen_gesture` с этим жестом.
    BeginStep(Box<PlannedStep>),
    /// Все шаги выполнены.
    AllDone,
    /// Прогон провален (причина готовым текстом).
    Failed(String),
}

/// Где находится текущий шаг.
enum StepPhase {
    Waiting { deadline: Instant },
    Settling { until: Instant },
}

struct ActiveStep {
    step: PlannedStep,
    baseline: ScreenProbe,
    phase: StepPhase,
}

/// Исполнитель плана.
pub struct StepRunner {
    pending: std::vec::IntoIter<PlannedStep>,
    active: Option<ActiveStep>,
    timing: StepTiming,
    /// Снимок на начало плана: от него считаются сквозные отказы.
    plan_start: ScreenProbe,
    records: Vec<StepRecord>,
    finished: bool,
}

impl StepRunner {
    pub fn new(steps: Vec<PlannedStep>, timing: StepTiming, plan_start: ScreenProbe) -> Self {
        Self {
            pending: steps.into_iter(),
            active: None,
            timing,
            plan_start,
            records: Vec::new(),
            finished: false,
        }
    }

    /// Записи выполненных (и последнего проваленного) шагов.
    pub fn records(&self) -> &[StepRecord] {
        &self.records
    }

    /// Имена шагов, до которых дело не дошло (включая текущий).
    pub fn unfinished_names(&mut self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.active.iter().map(|active| active.step.name).collect();
        names.extend(self.pending.by_ref().map(|step| step.name));
        names
    }

    /// Один тик. После `AllDone` и `Failed` больше ничего не делает.
    pub fn tick(&mut self, now: Instant, probe: &ScreenProbe) -> RunnerTick {
        if self.finished {
            return RunnerTick::Waiting;
        }
        if let Some(reason) = self.fatal_reason(probe) {
            return self.fail(reason);
        }
        let Some(active) = self.active.as_mut() else {
            return self.begin_next(now, probe);
        };
        let delta = ProbeDelta::between(&active.baseline, probe);
        // Перерост точного счётчика (лишний тап, лишнее намерение камере) — провал сразу, ждать нечего.
        if active.step.expect.is_overshot(&delta) {
            return self.fail_active(&delta, "лишнее");
        }
        match active.phase {
            StepPhase::Waiting { deadline } => {
                if active.step.expect.is_reached(&delta) {
                    active.phase = StepPhase::Settling {
                        until: now + self.timing.settle,
                    };
                } else if now >= deadline {
                    return self.fail_active(&delta, "срок вышел");
                }
                RunnerTick::Waiting
            }
            StepPhase::Settling { until } => {
                if now < until {
                    return RunnerTick::Waiting;
                }
                // Выдержка прошла: теперь строго — и «нужное случилось», и «лишнего нет».
                if !active.step.expect.violations(&delta).is_empty() {
                    return self.fail_active(&delta, "после выдержки");
                }
                self.complete_active(&delta);
                self.begin_next(now, probe)
            }
        }
    }

    fn begin_next(&mut self, now: Instant, probe: &ScreenProbe) -> RunnerTick {
        let Some(step) = self.pending.next() else {
            self.finished = true;
            return RunnerTick::AllDone;
        };
        self.active = Some(ActiveStep {
            step: step.clone(),
            baseline: *probe,
            phase: StepPhase::Waiting {
                deadline: now + self.timing.timeout,
            },
        });
        RunnerTick::BeginStep(Box::new(step))
    }

    fn complete_active(&mut self, delta: &ProbeDelta) {
        if let Some(active) = self.active.take() {
            self.records.push(StepRecord {
                name: active.step.name,
                ok: true,
                inferred: active.step.inferred.is_some(),
                observed: describe_delta(delta),
            });
        }
    }

    /// Проваливает текущий шаг: запись с «ожидали X, увидели Y» и готовая причина.
    fn fail_active(&mut self, delta: &ProbeDelta, when: &str) -> RunnerTick {
        let Some(active) = self.active.take() else {
            return self.fail("шаг провален, но активного шага нет".to_owned());
        };
        let problems = active.step.expect.violations(delta);
        // Причина идёт первой: бюджет строки результата режет нарушение с хвоста, а не с головы.
        let mut reason = format!(
            "шаг {} ({when}): {}. Цель шага: {}",
            active.step.name,
            problems.join(", "),
            active.step.purpose,
        );
        let intent_sent_but_no_trigger = delta.camera_intents > 0 && delta.focus_triggers == 0;
        if intent_sent_but_no_trigger && expects_trigger(&active.step.expect) {
            reason.push_str(". Намерение ушло в движок, но триггер до HAL не дошёл");
        }
        self.records.push(StepRecord {
            name: active.step.name,
            ok: false,
            inferred: active.step.inferred.is_some(),
            observed: format!("{reason}. Видно: {}", describe_delta(delta)),
        });
        self.fail(reason)
    }

    fn fail(&mut self, reason: String) -> RunnerTick {
        self.finished = true;
        RunnerTick::Failed(reason)
    }

    /// Сквозные отказы по приращению от начала плана.
    fn fatal_reason(&self, probe: &ScreenProbe) -> Option<String> {
        let since_start = ProbeDelta::between(&self.plan_start, probe);
        if since_start.focus_trigger_failures > 0 {
            return Some(format!(
                "HAL отверг {} триггер(ов) фокуса (focus_trigger_failures): тап дошёл до движка, но не до HAL",
                since_start.focus_trigger_failures
            ));
        }
        if since_start.camera_intents_lost > 0 {
            return Some(format!(
                "очередь движка не приняла {} намерение(й) экрана камере (очередь полна, движок остановлен или его нет)",
                since_start.camera_intents_lost
            ));
        }
        None
    }
}

fn expects_trigger(expectation: &Expectation) -> bool {
    expectation
        .counts()
        .iter()
        .any(|(metric, _)| *metric == super::screen_expect::Metric::FocusTriggers)
}

/// Приращения одной строкой для отчёта.
pub fn describe_delta(delta: &ProbeDelta) -> String {
    format!(
        "тапы {}, долгие {}, свайпы {}, кнопки {}, намерения камере {}, триггеры {}, снимки {}, блокировка {:?}, EV {:+.2}",
        delta.taps,
        delta.long_presses,
        delta.pans,
        delta.button_presses,
        delta.camera_intents,
        delta.focus_triggers,
        delta.capture_requests,
        delta.lock,
        delta.exposure_bias_ev,
    )
}

#[cfg(test)]
#[path = "screen_runner_tests.rs"]
mod tests;
