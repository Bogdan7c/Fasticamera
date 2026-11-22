//! Привязка слота кольца к кадру и стадии ISP кадра с учётом расписания статистики AWB.

use isp_vulkan::viewfinder::{IspStage, IspStageSet};

use super::super::exposure::ExposureSlotFrame;
use super::super::statistics_schedule::StatisticsDecision;
use super::super::white_balance::{AwbStream, StatisticsPlan};
use crate::viewfinder::AwbFrameIdentity;

/// Чей кадр ISP записал в слот: по этой метке статистика слота публикуется и отдаётся AWB своего потока.
/// Без потока статистику чужой камеры можно было бы принять за свою (таблица локуса у камер разная).
#[derive(Debug, Clone, Copy)]
pub(super) struct SlotFrame {
    pub(super) identity: AwbFrameIdentity,
    /// Чем освещён кадр: кадр со светодиодом статистику AE и AWB видоискателя не питает (14d).
    pub(super) light: camera_contract::flash::FrameLight,
    pub(super) stream: AwbStream,
    /// Кому из потребителей отдать статистику слота (решение общего расписания кадра).
    pub(super) statistics: StatisticsDecision,
    /// Поток и факты замера AE (`None` — AE выключен).
    pub(super) exposure: Option<ExposureSlotFrame>,
}

/// Стадии ISP кадра: включённые настройками, без статистики, если общее расписание (AWB + AE) её пропускает.
/// Пропуск стадии — это другой вариант пайплайна из кэша `isp-vulkan` (обе версии живут в кэше), а не
/// пересборка.
pub(super) fn effective_stages(configured: IspStageSet, statistics: StatisticsPlan) -> IspStageSet {
    match statistics {
        StatisticsPlan::Compute => configured,
        StatisticsPlan::Skip => configured.without(IspStage::AwbStatistics),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_statistics_drop_only_that_stage() {
        let all = IspStageSet::ALL;
        assert_eq!(effective_stages(all, StatisticsPlan::Compute), all);
        let skipped = effective_stages(all, StatisticsPlan::Skip);
        assert!(!skipped.contains(IspStage::AwbStatistics));
        assert!(skipped.contains(IspStage::Color) && skipped.contains(IspStage::Tone));
        // Стадия, выключенная командой, остаётся выключенной при любом расписании.
        let already_off = all.without(IspStage::AwbStatistics);
        assert_eq!(effective_stages(already_off, StatisticsPlan::Compute), already_off);
    }
}
