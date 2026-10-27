//! Сход на ближнюю линию кадра пропуском одного vsync (решение 54).
//!
//! Владелец — планировщик линии кадра (`present_target::PresentTargetPlanner`). Вход на каждом
//! спланированном кадре — «была ли выполнима линия ближе выбранной, которую не дал взять только
//! запрет двух кадров на один vsync»; выход — решение «этот тик пропустить».
//!
//! # Почему это работает
//! `NearestFeasible` метит строго позже линии прошлого кадра (больше чем на полпериода). Если тик
//! пропустить, линия прошлого кадра остаётся той, что была ДО пропуска, а линии следующего тика
//! уже на период дальше — значит, кадр следующего тика может метить на vsync на один ближе.
//! Один пропуск — одна ступень глубины. Камеру на пропущенном тике рендер не опрашивает, поэтому
//! при буфере видоискателя кадр не теряется на самом тике (он покажется следующим), но очередь
//! стала на кадр длиннее, и рано или поздно один кадр уйдёт переполнением — цена схода.
//!
//! # Правила
//! * Только при `NearestFeasible` и режиме `SkipOneVsync`.
//! * Серия: ближняя линия выполнима `depth_reduction_streak_frames` кадров подряд. Откат на
//!   предпочитаемую, выбор без запаса ближе или нечитаемые часы серию обнуляют.
//! * Не чаще одного схода за `depth_reduction_min_interval_ms` (по `CLOCK_MONOTONIC`).
//! * Сход засчитывается (`depth_reductions`), только если кадр после пропуска действительно
//!   выбрал линию ближе, чем до пропуска; иначе это лишь `skipped_ticks_for_depth`.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

use crate::config::{DepthReductionMode, PresentTargetConfig, PresentTimelinePolicy};

/// Что делать с тиком.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DepthStep {
    /// Планировать кадр как обычно.
    PlanFrame,
    /// Не рисовать и не показывать на этом тике (сход на ближнюю линию).
    SkipTick,
}

/// Чем кончился сход: проверяется по первому кадру после пропуска.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DepthOutcome {
    /// Кадр после пропуска выбрал линию ближе, чем до него.
    Reduced,
    /// Кадр после пропуска остался на той же глубине или ушёл глубже (оценка стоимости выросла).
    NotReduced,
}

/// Наблюдение за одним спланированным кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PlannedDepth {
    /// Сколько линий тика показываются раньше выбранной (0 — выбрана самая ближняя).
    pub(super) depth: usize,
    /// Была выполнима линия ближе выбранной, но её запретило правило «строго позже прошлой линии».
    pub(super) shallower_blocked_by_previous_target: bool,
}

#[derive(Debug)]
pub(super) struct DepthReducer {
    mode: DepthReductionMode,
    applies_to_policy: bool,
    streak_needed: u32,
    min_interval: Duration,
    /// Сколько кадров подряд ближняя линия была выполнима.
    streak: u32,
    /// Когда был последний пропуск (для паузы между сходами).
    last_skip_at: Option<MonotonicNanos>,
    /// Глубина последнего кадра (для проверки итога схода).
    last_depth: Option<usize>,
    /// Пропуск сделан, ждём первый кадр после него: глубина до пропуска.
    awaiting_outcome_from_depth: Option<usize>,
}

impl DepthReducer {
    pub(super) fn new(config: &PresentTargetConfig) -> Self {
        Self {
            mode: config.depth_reduction,
            applies_to_policy: matches!(config.policy, PresentTimelinePolicy::NearestFeasible { .. }),
            streak_needed: config.depth_reduction_streak_frames.max(1),
            min_interval: Duration::from_millis(config.depth_reduction_min_interval_ms),
            streak: 0,
            last_skip_at: None,
            last_depth: None,
            awaiting_outcome_from_depth: None,
        }
    }

    /// Решение перед планированием тика. `now` = `None` — часы не прочитались: не пропускаем.
    pub(super) fn step_for_tick(&mut self, now: Option<MonotonicNanos>) -> DepthStep {
        if !self.enabled() || self.streak < self.streak_needed {
            return DepthStep::PlanFrame;
        }
        let Some(now) = now else {
            return DepthStep::PlanFrame;
        };
        let cooled_down = self
            .last_skip_at
            .is_none_or(|last| elapsed_between(last, now) >= self.min_interval);
        if !cooled_down {
            return DepthStep::PlanFrame;
        }
        self.streak = 0;
        self.last_skip_at = Some(now);
        self.awaiting_outcome_from_depth = self.last_depth;
        DepthStep::SkipTick
    }

    /// Учитывает спланированный кадр; возвращает итог схода, если это первый кадр после пропуска.
    pub(super) fn observe_planned(&mut self, planned: Option<PlannedDepth>) -> Option<DepthOutcome> {
        let Some(planned) = planned else {
            // Откат на предпочитаемую или нечитаемые часы: серия прерывается.
            self.streak = 0;
            self.last_depth = None;
            return self
                .awaiting_outcome_from_depth
                .take()
                .map(|_| DepthOutcome::NotReduced);
        };
        self.streak = if planned.shallower_blocked_by_previous_target {
            self.streak.saturating_add(1)
        } else {
            0
        };
        self.last_depth = Some(planned.depth);
        self.awaiting_outcome_from_depth.take().map(|before| {
            if planned.depth < before {
                DepthOutcome::Reduced
            } else {
                DepthOutcome::NotReduced
            }
        })
    }

    fn enabled(&self) -> bool {
        self.applies_to_policy && self.mode == DepthReductionMode::SkipOneVsync
    }
}

/// `later − earlier` (ноль, если часы пошли назад).
fn elapsed_between(earlier: MonotonicNanos, later: MonotonicNanos) -> Duration {
    let nanos = later.as_nanos().saturating_sub(earlier.as_nanos());
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(0))
}

#[cfg(test)]
#[path = "present_depth_tests.rs"]
mod tests;
