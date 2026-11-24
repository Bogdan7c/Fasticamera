//! Как вариант видоискателя каталога ([`ViewfinderVariant`]) превращается в настройки работы `ViewfinderSaturation`.
//!
//! Работа сама умеет всё нужное: набор стадий (`stages`) и каденцию статистики AWB
//! (`awb_statistics_every_n_frames`). Здесь только таблица «вариант каталога → эти два поля»; всё остальное
//! (размер кадра, слоты, охват) остаётся продуктовым.

use isp_vulkan::saturation::ViewfinderSaturationConfig;
use isp_vulkan::viewfinder::{IspStage, IspStageSet};

use super::catalog::{AblatedStage, ViewfinderVariant};

/// Каденция «статистика AWB на каждом кадре».
const AWB_EVERY_FRAME: u32 = 1;

/// Стадия каталога → стадия ISP.
fn isp_stage(stage: AblatedStage) -> IspStage {
    match stage {
        AblatedStage::Levels => IspStage::Levels,
        AblatedStage::LensShading => IspStage::LensShading,
        AblatedStage::WhiteBalance => IspStage::WhiteBalance,
        AblatedStage::Demosaic => IspStage::Demosaic,
        AblatedStage::Color => IspStage::Color,
        AblatedStage::GamutCompression => IspStage::GamutCompression,
        AblatedStage::Tone => IspStage::Tone,
        AblatedStage::LookColor => IspStage::LookColor,
        AblatedStage::LookLut => IspStage::LookLut,
        AblatedStage::OutputGamutCompression => IspStage::OutputGamutCompression,
        AblatedStage::Oetf => IspStage::Oetf,
    }
}

/// Набор стадий «пол цены»: остаются `Levels` и `Demosaic` (чтение всей ячейки 2×2 и запись картинки).
fn minimal_stages() -> IspStageSet {
    IspStage::ALL
        .into_iter()
        .filter(|stage| !matches!(stage, IspStage::Levels | IspStage::Demosaic))
        .fold(IspStageSet::ALL, IspStageSet::without)
}

/// Настройки работы для варианта: `base` (продуктовые) с изменёнными стадиями и каденцией AWB.
pub fn configure(variant: ViewfinderVariant, base: ViewfinderSaturationConfig) -> ViewfinderSaturationConfig {
    match variant {
        ViewfinderVariant::Product => base,
        ViewfinderVariant::AwbNever => ViewfinderSaturationConfig {
            stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
            ..base
        },
        ViewfinderVariant::AwbEveryFrame => ViewfinderSaturationConfig {
            awb_statistics_every_n_frames: AWB_EVERY_FRAME,
            ..base
        },
        ViewfinderVariant::Without { stage } => ViewfinderSaturationConfig {
            stages: IspStageSet::ALL.without(isp_stage(stage)),
            ..base
        },
        ViewfinderVariant::Minimal => ViewfinderSaturationConfig {
            stages: minimal_stages(),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ViewfinderSaturationConfig {
        ViewfinderSaturationConfig::default()
    }

    fn enabled(stages: IspStageSet) -> Vec<IspStage> {
        IspStage::ALL
            .into_iter()
            .filter(|stage| stages.contains(*stage))
            .collect()
    }

    #[test]
    fn product_variant_is_the_unchanged_base() {
        let config = configure(ViewfinderVariant::Product, base());
        assert_eq!(config.stages, IspStageSet::ALL);
        assert_eq!(
            config.awb_statistics_every_n_frames,
            base().awb_statistics_every_n_frames
        );
    }

    #[test]
    fn awb_variants_change_only_statistics() {
        let never = configure(ViewfinderVariant::AwbNever, base());
        assert!(!never.stages.contains(IspStage::AwbStatistics));
        assert_eq!(
            enabled(never.stages).len(),
            IspStage::ALL.len() - 1,
            "остальные стадии на месте"
        );
        assert!(!(0..16).any(|frame| never.statistics_due(frame) && never.stages.contains(IspStage::AwbStatistics)));
        let every = configure(ViewfinderVariant::AwbEveryFrame, base());
        assert_eq!(every.stages, IspStageSet::ALL);
        assert!(
            (0..16).all(|frame| every.statistics_due(frame)),
            "статистика на каждом кадре"
        );
    }

    #[test]
    fn without_removes_exactly_the_named_stage_for_every_ablated_stage() {
        let all = [
            (AblatedStage::Levels, IspStage::Levels),
            (AblatedStage::LensShading, IspStage::LensShading),
            (AblatedStage::WhiteBalance, IspStage::WhiteBalance),
            (AblatedStage::Demosaic, IspStage::Demosaic),
            (AblatedStage::Color, IspStage::Color),
            (AblatedStage::GamutCompression, IspStage::GamutCompression),
            (AblatedStage::Tone, IspStage::Tone),
            (AblatedStage::LookColor, IspStage::LookColor),
            (AblatedStage::LookLut, IspStage::LookLut),
            (AblatedStage::OutputGamutCompression, IspStage::OutputGamutCompression),
            (AblatedStage::Oetf, IspStage::Oetf),
        ];
        assert_eq!(all.len() + 1, IspStage::ALL.len(), "все стадии, кроме статистики AWB");
        for (ablated, removed) in all {
            let config = configure(ViewfinderVariant::Without { stage: ablated }, base());
            let expected: Vec<IspStage> = IspStage::ALL.into_iter().filter(|stage| *stage != removed).collect();
            assert_eq!(enabled(config.stages), expected, "{ablated:?}");
            assert_eq!(
                config.awb_statistics_every_n_frames,
                base().awb_statistics_every_n_frames
            );
        }
    }

    #[test]
    fn minimal_keeps_only_levels_and_demosaic() {
        let config = configure(ViewfinderVariant::Minimal, base());
        assert_eq!(enabled(config.stages), vec![IspStage::Levels, IspStage::Demosaic]);
    }
}
