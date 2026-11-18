//! Привязка слота кольца к кадру и стадии ISP кадра с учётом расписания статистики AWB.

use isp_vulkan::viewfinder::{IspStage, IspStageSet};

use super::super::white_balance::{AwbStream, FramePlan, StatisticsPlan};
use crate::viewfinder::AwbFrameIdentity;

/// Чей кадр ISP записал в слот: по этой метке статистика слота публикуется и отдаётся AWB своего потока.
/// Без потока статистику чужой камеры можно было бы принять за свою (таблица локуса у камер разная).
#[derive(Debug, Clone, Copy)]
pub(super) struct SlotFrame {
    pub(super) identity: AwbFrameIdentity,
    pub(super) stream: AwbStream,
}

/// Стадии ISP кадра: включённые настройками, без статистики AWB, если расписание её пропускает.
/// Пропуск стадии — это другой вариант пайплайна из кэша `isp-vulkan` (обе версии живут в кэше), а не
/// пересборка.
pub(super) fn effective_stages(configured: IspStageSet, plan: &FramePlan) -> IspStageSet {
    match plan.statistics {
        StatisticsPlan::Compute => configured,
        StatisticsPlan::Skip => configured.without(IspStage::AwbStatistics),
    }
}

#[cfg(test)]
mod tests {
    use isp_model::color::chromaticity::Chromaticity;

    use super::*;
    use crate::vulkan_backend::camera::pipelines::IspPresentation;

    fn plan(statistics: StatisticsPlan) -> FramePlan {
        FramePlan {
            stream: AwbStream(1),
            white: Chromaticity::D65,
            presentation: IspPresentation::Show,
            statistics,
        }
    }

    #[test]
    fn skipped_statistics_drop_only_that_stage() {
        let all = IspStageSet::ALL;
        assert_eq!(effective_stages(all, &plan(StatisticsPlan::Compute)), all);
        let skipped = effective_stages(all, &plan(StatisticsPlan::Skip));
        assert!(!skipped.contains(IspStage::AwbStatistics));
        assert!(skipped.contains(IspStage::Color) && skipped.contains(IspStage::Tone));
        // Стадия, выключенная командой, остаётся выключенной при любом расписании.
        let already_off = all.without(IspStage::AwbStatistics);
        assert_eq!(
            effective_stages(already_off, &plan(StatisticsPlan::Compute)),
            already_off
        );
    }
}
