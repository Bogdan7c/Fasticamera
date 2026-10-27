//! Время GPU по проходам стенда: метки `vkCmdWriteTimestamp2` между проходами (решение 45).
//!
//! В отличие от слитого прохода видоискателя, здесь проходы отдельные и разделены барьерами, поэтому
//! отрезок между соседними метками — честное время одного прохода.

use gpu_vulkan::gpu_timing::{TimestampCollect, TimestampSpans};

use super::demosaic::MenonPass;

/// Проход стенда (единица замера).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeadroomPass {
    /// 0. Уровни и баланс белого.
    Levels,
    /// 1–7 (с 2а/2б). Проходы Menon 2007.
    Demosaic(MenonPass),
    /// Временной шумодав.
    TemporalDenoise,
    /// Цвет → охват → тон → OETF → приёмник кадра.
    Output,
}

impl HeadroomPass {
    /// Все проходы в порядке исполнения (шумодав — если включён).
    pub const ORDER: [Self; 11] = [
        Self::Levels,
        Self::Demosaic(MenonPass::DirectionalGreen),
        Self::Demosaic(MenonPass::ColorDifferences),
        Self::Demosaic(MenonPass::Decision),
        Self::Demosaic(MenonPass::ChromaAtGreen),
        Self::Demosaic(MenonPass::ChromaAtOpposite),
        Self::Demosaic(MenonPass::RefineGreen),
        Self::Demosaic(MenonPass::RefineChromaAtGreen),
        Self::Demosaic(MenonPass::RefineChromaAtOpposite),
        Self::TemporalDenoise,
        Self::Output,
    ];

    /// Короткое имя (метка «конец прохода»).
    pub fn label(self) -> &'static str {
        match self {
            Self::Levels => "levels",
            Self::Demosaic(pass) => pass.label(),
            Self::TemporalDenoise => "temporal_denoise",
            Self::Output => "output",
        }
    }

    /// Номер метки «конец прохода» (метка 0 — начало кадра).
    pub(crate) fn point_index(self) -> usize {
        Self::ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .map_or(0, |position| position + 1)
    }

    fn from_label(label: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|pass| pass.label() == label)
    }
}

/// Имя метки начала кадра.
pub(crate) const FRAME_START_LABEL: &str = "frame_start";
/// Номер метки начала кадра (первая в [`point_labels`]).
pub(crate) const FRAME_START_POINT: usize = 0;

/// Имена всех меток слота: начало кадра + конец каждого прохода.
pub(crate) fn point_labels() -> Vec<&'static str> {
    std::iter::once(FRAME_START_LABEL)
        .chain(HeadroomPass::ORDER.iter().map(|pass| pass.label()))
        .collect()
}

/// Время одного прохода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassTiming {
    /// Проход.
    pub pass: HeadroomPass,
    /// Время GPU, нс.
    pub duration_ns: u64,
}

/// Время кадра по проходам.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadroomTimings {
    /// По проходам в порядке исполнения (выключенный шумодав отсутствует).
    pub passes: Vec<PassTiming>,
    /// От начала кадра до конца выхода, нс.
    pub total_ns: u64,
}

/// Итог сбора меток слота (без ожиданий GPU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadroomTimingCollect {
    /// Метки готовы.
    Ready(HeadroomTimings),
    /// GPU ещё не дошёл до меток: спросить позже.
    NotReady,
    /// В слот ничего не записано (кадр не записывался или был пропущен).
    NothingRecorded,
    /// Очередь не поддерживает метки.
    Unsupported,
}

/// Переводит отрезки «метка → метка» в проходы: отрезок принадлежит проходу, которым он кончается.
pub(crate) fn timings_from_collect(collect: TimestampCollect) -> HeadroomTimingCollect {
    match collect {
        TimestampCollect::Ready(spans) => HeadroomTimingCollect::Ready(timings_from_spans(&spans)),
        TimestampCollect::NotReady => HeadroomTimingCollect::NotReady,
        TimestampCollect::NothingRecorded => HeadroomTimingCollect::NothingRecorded,
        TimestampCollect::Unsupported => HeadroomTimingCollect::Unsupported,
    }
}

fn timings_from_spans(spans: &TimestampSpans) -> HeadroomTimings {
    let passes = spans
        .spans
        .iter()
        .filter_map(|span| {
            HeadroomPass::from_label(span.to).map(|pass| PassTiming {
                pass,
                duration_ns: span.duration_ns,
            })
        })
        .collect();
    HeadroomTimings {
        passes,
        total_ns: spans.total_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpu_vulkan::gpu_timing::TimestampSpan;

    #[test]
    fn labels_are_unique_and_point_indices_follow_order() {
        let labels = point_labels();
        assert_eq!(labels.len(), 12);
        for (index, label) in labels.iter().enumerate() {
            assert!(!labels[..index].contains(label), "повтор метки {label}");
        }
        assert_eq!(HeadroomPass::Levels.point_index(), 1);
        assert_eq!(HeadroomPass::Output.point_index(), 11);
        assert_eq!(labels[HeadroomPass::TemporalDenoise.point_index()], "temporal_denoise");
    }

    #[test]
    fn spans_map_to_the_pass_they_end_with_and_skipped_pass_is_absent() {
        let span = |from, to, duration_ns| TimestampSpan { from, to, duration_ns };
        let spans = TimestampSpans {
            spans: vec![
                span("frame_start", "levels", 100),
                span("levels", "menon_green", 200),
                // Шумодав выключен: отрезок от последнего прохода демозаика сразу к выходу.
                span("menon_refine_chroma_opposite", "output", 300),
            ],
            total_ns: 600,
        };
        let HeadroomTimingCollect::Ready(timings) = timings_from_collect(TimestampCollect::Ready(spans)) else {
            panic!("ожидались готовые метки");
        };
        assert_eq!(
            timings.passes,
            vec![
                PassTiming {
                    pass: HeadroomPass::Levels,
                    duration_ns: 100
                },
                PassTiming {
                    pass: HeadroomPass::Demosaic(MenonPass::DirectionalGreen),
                    duration_ns: 200
                },
                PassTiming {
                    pass: HeadroomPass::Output,
                    duration_ns: 300
                },
            ]
        );
        assert_eq!(timings.total_ns, 600);
        assert_eq!(
            timings_from_collect(TimestampCollect::NotReady),
            HeadroomTimingCollect::NotReady
        );
    }
}
