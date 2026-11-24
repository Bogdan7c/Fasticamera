//! Как вариант видоискателя каталога ([`ViewfinderVariant`]) превращается в настройки работы `ViewfinderSaturation`.
//!
//! Работа сама умеет всё нужное: набор стадий (`stages`) и каденцию статистики AWB
//! (`awb_statistics_every_n_frames`). Здесь только таблица «вариант каталога → эти два поля»; всё остальное
//! (размер кадра, слоты, охват) остаётся продуктовым.

use camera_contract::viewfinder_crop::{CropAspect, ViewfinderCrop, ZoomFactor};
use isp_vulkan::saturation::ViewfinderSaturationConfig;
use isp_vulkan::viewfinder::{AwbReductionPolicy, IspStage, IspStageSet, WorkgroupSizeConfig};

use super::catalog::{AblatedStage, ViewfinderVariant};
use super::viewfinder_tuning::{AwbReductionChoice, AwbSchedule, ViewfinderTuning};

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
        ViewfinderVariant::Tuned(tuning) => apply_tuning(tuning, base),
    }
}

/// Опытные настройки поверх продуктовых: каденция AWB, сведение AWB, размер группы, набор стадий.
#[expect(
    clippy::expect_used,
    reason = "строки каталога статичны: неверное приближение — ошибка кода каталога, молча мерить продукт нельзя"
)]
fn apply_tuning(tuning: ViewfinderTuning, base: ViewfinderSaturationConfig) -> ViewfinderSaturationConfig {
    let mut config = base;
    match tuning.awb_schedule {
        AwbSchedule::Product => {}
        AwbSchedule::EveryFrame => config.awb_statistics_every_n_frames = AWB_EVERY_FRAME,
        AwbSchedule::Never => config.stages = config.stages.without(IspStage::AwbStatistics),
    }
    if tuning.minimal_stages {
        config.stages = minimal_stages();
    }
    match tuning.awb_reduction {
        AwbReductionChoice::Default => {}
        AwbReductionChoice::SharedMemoryOnly => config.isp.awb_reduction = AwbReductionPolicy::SharedMemoryOnly,
        AwbReductionChoice::SubgroupWhenSupported => {
            config.isp.awb_reduction = AwbReductionPolicy::SubgroupWhenSupported;
        }
    }
    if tuning.crop_zoom_permille != 0 {
        // Диагностический кроп: значение вне допустимого — ошибка настройки каталога (его строки статичны), поэтому
        // падаем громко, а не молча меряем продукт.
        let zoom = ZoomFactor::new(tuning.crop_zoom_permille as f32 / 1000.0)
            .expect("приближение каталога стенда должно быть не меньше 1");
        config.crop = ViewfinderCrop::centered(CropAspect::SensorNative, zoom);
    }
    if let Some(shape) = tuning.workgroup {
        config.isp.workgroup_size = WorkgroupSizeConfig { x: shape.x, y: shape.y };
    }
    config
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

    #[test]
    fn tuned_variant_changes_only_what_the_tuning_names() {
        let untouched = configure(ViewfinderVariant::Tuned(ViewfinderTuning::PRODUCT), base());
        assert_eq!(untouched.stages, IspStageSet::ALL);
        assert_eq!(untouched.isp, base().isp, "без изменений — настройки ISP продуктовые");
        assert_eq!(
            untouched.awb_statistics_every_n_frames,
            base().awb_statistics_every_n_frames
        );

        let shaped = configure(
            ViewfinderVariant::Tuned(ViewfinderTuning::PRODUCT.workgroup(4, 32).subgroup_awb()),
            base(),
        );
        assert_eq!(shaped.isp.workgroup_size, WorkgroupSizeConfig { x: 4, y: 32 });
        assert_eq!(shaped.isp.awb_reduction, AwbReductionPolicy::SubgroupWhenSupported);
        assert_eq!(shaped.stages, IspStageSet::ALL, "стадии не тронуты");

        let never = configure(
            ViewfinderVariant::Tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never)),
            base(),
        );
        assert!(!never.stages.contains(IspStage::AwbStatistics));
        let every = configure(
            ViewfinderVariant::Tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::EveryFrame)),
            base(),
        );
        assert!((0..16).all(|frame| every.statistics_due(frame)));
        let minimal = configure(ViewfinderVariant::Tuned(ViewfinderTuning::PRODUCT.minimal()), base());
        assert_eq!(enabled(minimal.stages), vec![IspStage::Levels, IspStage::Demosaic]);
    }
}
