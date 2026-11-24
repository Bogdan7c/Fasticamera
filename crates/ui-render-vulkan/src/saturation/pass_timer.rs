//! Времена проходов кадра: timestamp-точки GPU по слотам + журнал `PassTimeLog` ядра.
//!
//! # Как это устроено
//! В кадре ставятся до четырёх точек ([`PassPoint`]); время прохода — разница двух соседних записанных точек.
//! Какие точки записаны, зависит от состава кадра ([`super::FrameComposition`]), поэтому метки проходов
//! (`compose`/`clear`, `pyramid`, `ui`) берутся из состава: число отрезков всегда равно числу меток.
//! После fence кадра [`PassTimer::collect_after_fence`] забирает отрезки слота и кладёт их в журнал;
//! нет поддержки timestamp-ов или метки не готовы — кадр уходит в журнал как «не измеренный».

use std::time::Duration;

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::saturation::{PassTimeLog, PassTimeSummary};

/// Точки кадра, в которых ставится метка времени.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PassPoint {
    /// Начало кадра (до подготовки цели).
    Start = 0,
    /// Закончен первый проход: компоновщик или очистка цели (для `UiOnly`).
    FirstPassDone = 1,
    /// Закончена пирамида фона (или её нет — тогда сразу после первого прохода).
    PyramidDone = 2,
    /// Закончен проход UI.
    UiDone = 3,
}

/// Имена точек для `GpuTimestamps` (индекс = значение [`PassPoint`]).
const POINT_LABELS: [&str; 4] = ["start", "first_pass_done", "pyramid_done", "ui_done"];

/// Метки времени GPU по слотам и журнал времён проходов.
pub(crate) struct PassTimer {
    timestamps: GpuTimestamps,
    log: PassTimeLog,
    /// Заранее выделенный буфер длительностей: сбор после fence не выделяет память под отрезки сам.
    durations: Vec<Duration>,
}

impl PassTimer {
    /// `labels` — имена проходов в порядке записи; `window_frames` — сколько последних кадров помнить.
    pub(crate) fn new(
        context: &GpuContext,
        frame_slots: usize,
        labels: &[&'static str],
        window_frames: usize,
    ) -> Result<Self, GpuError> {
        Ok(Self {
            timestamps: GpuTimestamps::new(context, frame_slots, &POINT_LABELS)?,
            log: PassTimeLog::new(labels, window_frames),
            durations: Vec::with_capacity(labels.len()),
        })
    }

    /// Сбрасывает точки слота: в начале командного буфера, вне рендеринга.
    pub(crate) fn record_reset(&mut self, command_buffer: vk::CommandBuffer, slot: usize) -> Result<(), GpuError> {
        self.timestamps.record_reset(command_buffer, slot)
    }

    /// Ставит метку `point` слота (после всех предыдущих команд кадра).
    pub(crate) fn record_point(
        &mut self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        point: PassPoint,
    ) -> Result<(), GpuError> {
        self.timestamps
            .record_point(command_buffer, slot, point as usize, TimestampStage::AllCommandsDone)
    }

    /// Fence кадра слота сигнален: забирает отрезки и кладёт в журнал. Метки не готовы или не поддерживаются —
    /// кадр засчитывается как не измеренный (ошибкой это не считается: так устроено устройство).
    pub(crate) fn collect_after_fence(&mut self, slot: usize) -> Result<(), GpuError> {
        match self.timestamps.try_collect(slot)? {
            TimestampCollect::Ready(spans) => {
                self.durations.clear();
                self.durations
                    .extend(spans.spans.iter().map(|span| Duration::from_nanos(span.duration_ns)));
                self.log.record_frame(&self.durations);
            }
            TimestampCollect::NotReady | TimestampCollect::NothingRecorded | TimestampCollect::Unsupported => {
                self.log.record_unmeasured_frame();
            }
        }
        Ok(())
    }

    /// Итог по накопленным кадрам.
    pub(crate) fn summary(&self) -> PassTimeSummary {
        self.log.summary()
    }

    /// Забыть накопленное (например, после прогрева).
    pub(crate) fn clear(&mut self) {
        self.log.clear();
    }
}
