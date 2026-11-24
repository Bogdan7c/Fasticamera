//! Замер CPU шагов движка снимков на потоке рендера (`StillStepCpu`).
//!
//! Отдельный модуль, чтобы `engine_steps` не разрастался: здесь только «засечь время шага и записать».
//! Часы — те же `StillClock`, что у движка (в тестах — ручные, их можно сдвинуть внутри шага).

use std::time::Instant;

use crate::still_diagnostics::{DurationStat, StillStepCpu};

use super::engine::StillEngine;
use super::gpu::StillGpu;

/// Какой шаг движка замеряется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StillStep {
    Advance,
    Choosing,
    CapturePoll,
    CaptureStart,
    Develop,
    Present,
    DevelopBegin,
    EncoderPresentStart,
    /// Создание GPU снимков (только когда оно происходит).
    GpuCreation,
}

impl StillStep {
    /// Ячейка счётчика этого шага.
    fn slot(self, timings: &mut StillStepCpu) -> &mut DurationStat {
        match self {
            Self::Advance => &mut timings.advance,
            Self::Choosing => &mut timings.choosing,
            Self::CapturePoll => &mut timings.capture_poll,
            Self::CaptureStart => &mut timings.capture_start,
            Self::Develop => &mut timings.develop,
            Self::Present => &mut timings.present,
            Self::DevelopBegin => &mut timings.develop_begin,
            Self::EncoderPresentStart => &mut timings.encoder_present_start,
            Self::GpuCreation => &mut timings.gpu_creation,
        }
    }
}

impl<G: StillGpu> StillEngine<G> {
    /// Выполняет `run` и записывает, сколько он занял на часах движка.
    pub(super) fn timed<R>(&mut self, step: StillStep, run: impl FnOnce(&mut Self) -> R) -> R {
        let started = self.clock.now();
        let result = run(self);
        self.record_step_cpu(step, started);
        result
    }

    /// Записывает время шага, начатого в `started` (для мест, где шаг нельзя обернуть замыканием из-за
    /// заимствования GPU).
    pub(super) fn record_step_cpu(&mut self, step: StillStep, started: Instant) {
        let spent = self.clock.now().saturating_duration_since(started);
        step.slot(&mut self.counters.step_cpu)
            .observe(spent.as_secs_f64() * 1_000.0);
    }
}
