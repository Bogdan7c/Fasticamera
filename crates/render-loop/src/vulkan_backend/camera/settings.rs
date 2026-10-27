//! Настройки видоискателя, которые меняются на ходу командами (`ViewfinderCommand`).
//!
//! Владелец — бэкенд показа: настройки переживают пересоздание surface, а живая сессия показа
//! получает копию после каждой применённой команды. Здесь только правило «что меняет команда»
//! (чистая логика, проверяется без GPU).

use camera_contract::viewfinder_crop::ViewfinderCrop;
use isp_vulkan::viewfinder::IspStageSet;

use crate::config::{ViewfinderConfig, ViewfinderConfigError, ViewfinderPlacement};
use crate::viewfinder::{HeadroomBench, ViewfinderCommand, ViewfinderCommandOutcome, ViewfinderPainter};

/// Действующие настройки видоискателя.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewfinderSettings {
    /// Чем рисовать.
    pub painter: ViewfinderPainter,
    /// Включённые стадии ISP.
    pub stages: IspStageSet,
    /// Кадрирование.
    pub crop: ViewfinderCrop,
    /// Положение на экране (только из конфига).
    pub placement: ViewfinderPlacement,
    /// Стенд «запас под видео» (V4): на старте всегда выключен, включается только командой.
    pub headroom: HeadroomBench,
}

impl ViewfinderSettings {
    /// Стартовые настройки из конфига: все стадии ISP включены (продукт), стенд V4 выключен.
    pub fn from_config(config: &ViewfinderConfig) -> Result<Self, ViewfinderConfigError> {
        Ok(Self {
            painter: config.painter,
            stages: IspStageSet::ALL,
            crop: config.crop.to_crop()?,
            placement: config.placement,
            headroom: HeadroomBench::Disabled,
        })
    }

    /// Применяет команду. Отвергнутая команда настроек не меняет.
    pub fn apply(&mut self, command: ViewfinderCommand) -> ViewfinderCommandOutcome {
        if let Some(reason) = command.rejection_reason() {
            return ViewfinderCommandOutcome::Rejected { reason };
        }
        let before = *self;
        match command {
            ViewfinderCommand::SetPainter(painter) => self.painter = painter,
            ViewfinderCommand::SetIspStages(stages) => self.stages = stages,
            ViewfinderCommand::SetCrop(crop) => self.crop = crop,
            ViewfinderCommand::SetHeadroomBench(mode) => self.headroom = mode,
        }
        if *self == before {
            ViewfinderCommandOutcome::AlreadyInEffect
        } else {
            ViewfinderCommandOutcome::Applied
        }
    }
}

#[cfg(test)]
mod tests {
    use camera_contract::viewfinder_crop::{CropAspect, ZoomFactor};
    use isp_vulkan::viewfinder::IspStage;

    use super::*;

    fn defaults() -> ViewfinderSettings {
        ViewfinderSettings::from_config(&ViewfinderConfig::default()).expect("конфиг по умолчанию корректен")
    }

    #[test]
    fn defaults_are_the_product_isp_with_all_stages() {
        let settings = defaults();
        assert_eq!(settings.painter, ViewfinderPainter::Isp);
        assert_eq!(settings.stages, IspStageSet::ALL);
        assert_eq!(settings.crop, ViewfinderCrop::FULL_SENSOR);
    }

    #[test]
    fn headroom_bench_starts_disabled_and_toggles_without_touching_the_viewfinder() {
        let mut settings = defaults();
        assert_eq!(
            settings.headroom,
            HeadroomBench::Disabled,
            "стенд не продукт: по умолчанию выключен"
        );
        let enable = ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled);
        assert_eq!(settings.apply(enable), ViewfinderCommandOutcome::Applied);
        assert_eq!(settings.apply(enable), ViewfinderCommandOutcome::AlreadyInEffect);
        assert_eq!(settings.headroom, HeadroomBench::Enabled);
        assert_eq!(
            (settings.painter, settings.stages, settings.crop),
            (ViewfinderPainter::Isp, IspStageSet::ALL, ViewfinderCrop::FULL_SENSOR),
            "видоискатель работает как прежде"
        );
        assert_eq!(
            settings.apply(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.headroom, HeadroomBench::Disabled);
    }

    #[test]
    fn painter_switch_applies_once_and_then_is_already_in_effect() {
        let mut settings = defaults();
        let to_debug = ViewfinderCommand::SetPainter(ViewfinderPainter::DebugMosaic);
        assert_eq!(settings.apply(to_debug), ViewfinderCommandOutcome::Applied);
        assert_eq!(settings.painter, ViewfinderPainter::DebugMosaic);
        assert_eq!(settings.apply(to_debug), ViewfinderCommandOutcome::AlreadyInEffect);
        assert_eq!(
            settings.stages,
            IspStageSet::ALL,
            "смена рисовальщика стадии не трогает"
        );
    }

    #[test]
    fn stage_set_command_replaces_the_whole_set() {
        let mut settings = defaults();
        let without_tone = IspStageSet::ALL.without(IspStage::Tone);
        assert_eq!(
            settings.apply(ViewfinderCommand::SetIspStages(without_tone)),
            ViewfinderCommandOutcome::Applied
        );
        assert!(!settings.stages.contains(IspStage::Tone));
        assert!(settings.stages.contains(IspStage::Color));
        assert_eq!(
            settings.painter,
            ViewfinderPainter::Isp,
            "стадии не меняют рисовальщика"
        );
    }

    #[test]
    fn invalid_crop_is_rejected_and_changes_nothing() {
        let mut settings = defaults();
        let outcome = settings.apply(ViewfinderCommand::SetCrop(ViewfinderCrop {
            aspect: CropAspect::LongToShort { long: 1, short: 2 },
            zoom: ZoomFactor::NONE,
        }));
        assert!(
            matches!(outcome, ViewfinderCommandOutcome::Rejected { .. }),
            "{outcome:?}"
        );
        assert_eq!(settings, defaults());
        let square = ViewfinderCrop {
            aspect: CropAspect::SQUARE,
            zoom: ZoomFactor::NONE,
        };
        assert_eq!(
            settings.apply(ViewfinderCommand::SetCrop(square)),
            ViewfinderCommandOutcome::Applied
        );
        assert_eq!(settings.crop, square);
    }
}
