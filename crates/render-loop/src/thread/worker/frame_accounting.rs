//! Учёт кадра потоком рендера: план линии кадра, замеры CPU и GPU от бэкенда, записи о показах и
//! команды ADPF. Вынесено из `worker.rs` (размер модуля): здесь только перенос замеров бэкенда во
//! владельцев учёта (`PresentTargetTracker`, `AdpfReporter`, пейсинг), собственных решений нет.

use super::super::adpf::AdpfCommand;
use super::super::frame_clock::{FrameClock, FrameTick};
use super::super::monotonic_clock::{MonotonicTimer, SystemMonotonicTimer};
use super::super::present_target::TickPlan;
use super::{LOG_TARGET, RenderWorker};
use crate::backend::{DisplayBackend, FrameCpuTiming};

impl<B: DisplayBackend, C: FrameClock> RenderWorker<B, C> {
    /// Команда ADPF — состояние потока (не бэкенда): применяется сразу, между кадрами.
    pub(super) fn apply_adpf_command(&mut self, command: AdpfCommand) {
        let outcome = self.adpf.apply_command(command);
        self.status.adpf_commands.note(command, outcome);
        self.status.adpf = self.adpf.snapshot();
    }

    /// Планирует тик: линия кадра по политике конфига или пропуск ради схода на ближнюю линию.
    /// Часы не прочитались — планировщик откатывается на предпочитаемую линию и считает сбой
    /// (ошибка не глотается, а видна в статусе).
    pub(super) fn plan_present_target(&mut self, frame_index: u64, tick: &FrameTick) -> TickPlan {
        let now = SystemMonotonicTimer
            .now()
            .inspect_err(|error| tracing::warn!(target: LOG_TARGET, %error, "монотонные часы не прочитались"))
            .ok();
        self.present_target
            .plan_frame(frame_index, &tick.timelines, now, self.pacing.refresh_period())
    }

    /// Забирает у бэкенда замер CPU кадра (acquire, захват → сабмит) для оценки и диагностики;
    /// возвращает его же (ADPF вычитает из времени кадра ожидания).
    pub(super) fn account_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        let timing = self.guarded(DisplayBackend::take_frame_cpu_timing).flatten()?;
        self.present_target.record_cpu_timing(timing);
        Some(timing)
    }

    /// Времена прошлых кадров на GPU (приходят на 1–3 кадра позже): в оценку стоимости кадра и в ADPF.
    pub(super) fn account_frame_gpu_timings(&mut self) {
        for timing in self
            .guarded(DisplayBackend::drain_frame_gpu_timings)
            .unwrap_or_default()
        {
            self.present_target.record_gpu_timing(timing);
            self.adpf.gpu_timing(timing);
        }
    }

    /// Забирает у бэкенда новые времена показов и кладёт их в статистику пейсинга.
    /// Дёшево: вызывается на каждом кадре и перед событиями, которые уничтожают цепочку.
    pub(super) fn drain_past_presentations(&mut self) {
        let Some(presentations) = self.guarded(DisplayBackend::drain_past_presentations) else {
            return;
        };
        for presentation in presentations {
            self.status
                .note_presentation(presentation, self.config.recent_presentations_capacity);
            self.pacing
                .record_present(presentation.present_id, presentation.actual_present);
            self.present_target
                .record_presentation(&presentation, self.pacing.refresh_period());
        }
    }
}
