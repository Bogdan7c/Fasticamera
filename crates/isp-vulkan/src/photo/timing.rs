//! Время GPU по проходам проявления снимка: метки `vkCmdWriteTimestamp2` между проходами (сессия 16a).
//!
//! Постоянный прибор, выключенный по умолчанию: пока [`PhotoDeveloper::enable_gpu_timing`] не вызван, в командные
//! буферы не пишется ни одной метки. Стенд насыщения включает его, чтобы видеть цену каждого прохода на телефоне
//! (в `photo` раньше не было ни одного замера GPU, а на Adreno мс GPU в темпе vsync цену работы не показывают).
//!
//! # Что именно меряется
//! Снимок — 12 полос по 256 строк, и на 12 полос × 10 проходов меток не хватит (в слоте `GpuTimestamps` их ≤ 64).
//! Поэтому проходы разбираются на ОДНОЙ образцовой полосе (средней: у неё полный ореол сверху и снизу), а
//! остальные полосы идут двумя целыми отрезками «до образца» и «после образца». Полоса бывает только одного
//! размера (кроме краёв), поэтому цену одной полосы продукта даёт сумма проходов образца.
//!
//! ```text
//! начало ─ before_sample ─ [образец: levels, 8 × menon, output] ─ after_sample ─ finishing (миниатюра + копии)
//! ```
//! Отрезок принадлежит метке, которой он кончается (как в `quality::timing`).
//!
//! # Контракт слота
//! Слот один (проявитель проявляет один снимок за раз). `begin_development` сбрасывает метки на CPU; вызывать его
//! можно, только когда fence прошлого сабмита меток сигнален — стенд и продукт подтверждают полосы fence-ом перед
//! тем, как проявитель начнёт следующую проявку, так что условие выполнено само.

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampSpans, TimestampStage};

use crate::quality::demosaic::MenonPass;

/// Слот меток: проявитель один за раз.
const SLOT: usize = 0;

/// Метка времени (конец отрезка).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhotoGpuPoint {
    /// Начало проявки: все прежние команды очереди закончились (отрезка у неё нет).
    Start,
    /// Конец всех полос ДО образцовой (отрезок «до образца»).
    BeforeSample,
    /// Уровни и баланс белого образцовой полосы.
    Levels,
    /// Один из восьми проходов Menon образцовой полосы.
    Demosaic(MenonPass),
    /// Вывод образцовой полосы (матрицы, охват, тон, OETF, квантование).
    Output,
    /// Конец всех полос ПОСЛЕ образцовой (отрезок «после образца»).
    AfterSample,
    /// Миниатюра в шаге отделки: уменьшение и копия миниатюры в память хоста.
    Thumbnail,
    /// Остаток шага отделки: перевод основной картинки к копированию и её копия (JPEG).
    Finishing,
}

impl PhotoGpuPoint {
    /// Метки в порядке записи.
    pub const ORDER: [Self; 15] = [
        Self::Start,
        Self::BeforeSample,
        Self::Levels,
        Self::Demosaic(MenonPass::DirectionalGreen),
        Self::Demosaic(MenonPass::ColorDifferences),
        Self::Demosaic(MenonPass::Decision),
        Self::Demosaic(MenonPass::ChromaAtGreen),
        Self::Demosaic(MenonPass::ChromaAtOpposite),
        Self::Demosaic(MenonPass::RefineGreen),
        Self::Demosaic(MenonPass::RefineChromaAtGreen),
        Self::Demosaic(MenonPass::RefineChromaAtOpposite),
        Self::Output,
        Self::AfterSample,
        Self::Thumbnail,
        Self::Finishing,
    ];

    /// Короткое имя (метка отрезка в журнале стенда).
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::BeforeSample => "before_sample",
            Self::Levels => "levels",
            Self::Demosaic(pass) => pass.label(),
            Self::Output => "output",
            Self::AfterSample => "after_sample",
            Self::Thumbnail => "thumbnail",
            Self::Finishing => "finishing",
        }
    }

    /// Номер точки в слоте меток.
    fn index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or_default()
    }

    /// Стадия метки: начало ждёт ВСЁ прежнее (в том числе кадры в полёте), конец прохода — compute-шейдеры;
    /// отделка включает копии (передача), поэтому ждёт все команды.
    fn stage(self) -> TimestampStage {
        match self {
            Self::Start | Self::Thumbnail | Self::Finishing => TimestampStage::AllCommandsDone,
            _ => TimestampStage::ComputeShaderDone,
        }
    }

    /// Имена отрезков для журнала стенда в порядке записи (без `Start`: у неё отрезка нет).
    pub fn span_labels() -> Vec<&'static str> {
        Self::ORDER.iter().skip(1).map(|point| point.label()).collect()
    }
}

/// Длительности отрезков одной проявки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoGpuTimings {
    /// Отрезки в порядке записи: имя метки-конца и время GPU, нс.
    pub spans: Vec<(&'static str, u64)>,
    /// Полное время от начала проявки до конца отделки, нс.
    pub total_ns: u64,
}

/// Итог сбора меток (без ожидания GPU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotoGpuTimingCollect {
    /// Метки готовы.
    Ready(PhotoGpuTimings),
    /// GPU ещё не дошёл до меток.
    NotReady,
    /// Проявка не записывала меток (прибор выключен или проявка не дошла до шага).
    NothingRecorded,
    /// Очередь не поддерживает метки.
    Unsupported,
}

/// Место записываемого шага среди полос проявки (для расстановки меток).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SliceStep {
    /// Первая полоса: здесь сбрасываются метки.
    pub(crate) is_first: bool,
    /// Образцовая полоса: проходы разбираются по меткам.
    pub(crate) is_sample: bool,
    /// Последняя полоса вывода: после неё метка «после образца».
    pub(crate) is_last: bool,
}

impl SliceStep {
    /// Шаг `index` из `slice_count` полос вывода.
    pub(crate) fn new(index: usize, slice_count: usize) -> Self {
        Self {
            is_first: index == 0,
            is_sample: index == PhotoGpuTimer::sample_slice(slice_count),
            is_last: index + 1 == slice_count,
        }
    }
}

/// Метки проявителя: пул запросов и номер образцовой полосы.
pub(crate) struct PhotoGpuTimer {
    timestamps: GpuTimestamps,
}

impl PhotoGpuTimer {
    pub(crate) fn new(context: &GpuContext) -> Result<Self, GpuError> {
        let labels: Vec<&'static str> = PhotoGpuPoint::ORDER.iter().map(|point| point.label()).collect();
        Ok(Self {
            timestamps: GpuTimestamps::new(context, 1, &labels)?,
        })
    }

    /// Пишет метку, если прибор включён (`None` — ничего не пишет и ничего не стоит).
    pub(crate) fn mark_if_enabled(
        timer: &mut Option<Self>,
        command_buffer: vk::CommandBuffer,
        point: PhotoGpuPoint,
    ) -> Result<(), GpuError> {
        match timer {
            Some(timer) => timer.mark(command_buffer, point),
            None => Ok(()),
        }
    }

    /// Номер образцовой полосы: средняя (у неё ореол и сверху, и снизу, полный размер).
    pub(crate) fn sample_slice(slice_count: usize) -> usize {
        slice_count / 2
    }

    /// Сбрасывает метки и пишет начало проявки (первый шаг, в начале командного буфера).
    pub(crate) fn begin_development(&mut self, command_buffer: vk::CommandBuffer) -> Result<(), GpuError> {
        self.timestamps.record_reset(command_buffer, SLOT)?;
        self.mark(command_buffer, PhotoGpuPoint::Start)
    }

    /// Пишет метку конца прохода.
    pub(crate) fn mark(&mut self, command_buffer: vk::CommandBuffer, point: PhotoGpuPoint) -> Result<(), GpuError> {
        self.timestamps
            .record_point(command_buffer, SLOT, point.index(), point.stage())
    }

    /// Забирает метки; не ждёт GPU.
    pub(crate) fn collect(&mut self) -> Result<PhotoGpuTimingCollect, GpuError> {
        Ok(match self.timestamps.try_collect(SLOT)? {
            TimestampCollect::Ready(spans) => PhotoGpuTimingCollect::Ready(timings_from_spans(&spans)),
            TimestampCollect::NotReady => PhotoGpuTimingCollect::NotReady,
            TimestampCollect::NothingRecorded => PhotoGpuTimingCollect::NothingRecorded,
            TimestampCollect::Unsupported => PhotoGpuTimingCollect::Unsupported,
        })
    }
}

fn timings_from_spans(spans: &TimestampSpans) -> PhotoGpuTimings {
    PhotoGpuTimings {
        spans: spans.spans.iter().map(|span| (span.to, span.duration_ns)).collect(),
        total_ns: spans.total_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpu_vulkan::gpu_timing::TimestampSpan;

    #[test]
    fn labels_are_unique_and_indices_follow_order() {
        let labels: Vec<_> = PhotoGpuPoint::ORDER.iter().map(|point| point.label()).collect();
        for (index, label) in labels.iter().enumerate() {
            assert!(!labels[..index].contains(label), "повтор метки {label}");
        }
        for (index, point) in PhotoGpuPoint::ORDER.iter().enumerate() {
            assert_eq!(point.index(), index, "{}", point.label());
        }
        assert_eq!(PhotoGpuPoint::span_labels().len(), PhotoGpuPoint::ORDER.len() - 1);
    }

    #[test]
    fn sample_slice_is_the_middle_one_and_exists_for_every_slice_count() {
        for slice_count in 1..40 {
            assert!(PhotoGpuTimer::sample_slice(slice_count) < slice_count);
        }
        assert_eq!(PhotoGpuTimer::sample_slice(12), 6);
    }

    #[test]
    fn step_flags_mark_first_sample_and_last_slices() {
        let steps: Vec<SliceStep> = (0..5).map(|index| SliceStep::new(index, 5)).collect();
        assert_eq!(steps.iter().filter(|step| step.is_first).count(), 1);
        assert_eq!(steps.iter().position(|step| step.is_sample), Some(2));
        assert_eq!(steps.iter().position(|step| step.is_last), Some(4));
        let single = SliceStep::new(0, 1);
        assert!(single.is_first && single.is_sample && single.is_last);
    }

    #[test]
    fn spans_keep_the_label_they_end_with() {
        let spans = TimestampSpans {
            spans: vec![
                TimestampSpan {
                    from: "start",
                    to: "before_sample",
                    duration_ns: 7,
                },
                TimestampSpan {
                    from: "before_sample",
                    to: "levels",
                    duration_ns: 3,
                },
            ],
            total_ns: 10,
        };
        let timings = timings_from_spans(&spans);
        assert_eq!(timings.spans, vec![("before_sample", 7), ("levels", 3)]);
        assert_eq!(timings.total_ns, 10);
    }
}
