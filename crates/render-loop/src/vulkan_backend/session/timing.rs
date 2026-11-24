//! Замеры времени кадра и ожидание позднего захвата (сессия 07): вынесено из `session` без изменения поведения.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

use super::LOG_TARGET;
use crate::backend::{FrameLatchPlan, LatchWaitResult};
use crate::config::LateLatchMode;
use crate::thread::{MonotonicTimer, SleepUntilOutcome, SystemMonotonicTimer};

/// Ждёт момента позднего захвата перед опросом камеры, если это включено в плане и рисуется камера
/// (`draws_camera`; тестовому паттерну свежесть кадра камеры не нужна). Только ожидание CPU, не GPU.
pub(super) fn wait_for_latch(plan: &FrameLatchPlan, draws_camera: bool) -> LatchWaitResult {
    if plan.mode != LateLatchMode::BeforeCameraPoll || !draws_camera {
        return LatchWaitResult::NotRequested;
    }
    match plan.wait_until_latch() {
        SleepUntilOutcome::Reached { .. } | SleepUntilOutcome::AlreadyPassed { .. } => LatchWaitResult::Waited,
        SleepUntilOutcome::RefusedTooFar { remaining } => {
            // Не warn на каждый кадр: при `Preferred` с далёкой линией это штатно, отказы
            // считает диагностика (`latch_waits_refused`).
            tracing::debug!(
                target: LOG_TARGET,
                remaining_us = remaining.as_micros(),
                max_wait_us = plan.max_wait.as_micros(),
                "момент позднего захвата дальше предела ожидания: камера опрошена сразу"
            );
            LatchWaitResult::RefusedBeyondBudget
        }
        SleepUntilOutcome::ClockFailed(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "поздний захват без ожидания: часы не прочитались");
            LatchWaitResult::ClockFailed
        }
    }
}

/// «Сейчас» по монотонным часам; сбой чтения логируется и даёт `None` (замер пропускается).
pub(super) fn read_monotonic_clock() -> Option<MonotonicNanos> {
    SystemMonotonicTimer
        .now()
        .inspect_err(|error| tracing::warn!(target: LOG_TARGET, %error, "замер CPU кадра пропущен"))
        .ok()
}

/// Длительность между двумя замерами (ноль при обратном порядке); `None`, если замера нет.
pub(super) fn elapsed_between(earlier: Option<MonotonicNanos>, later: Option<MonotonicNanos>) -> Option<Duration> {
    let difference = later?.as_nanos().saturating_sub(earlier?.as_nanos());
    Some(Duration::from_nanos(u64::try_from(difference).unwrap_or(0)))
}
