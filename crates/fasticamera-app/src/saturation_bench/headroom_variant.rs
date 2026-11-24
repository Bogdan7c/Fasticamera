//! Опыт каталога → настройки цепочки V4: превращает данные [`HeadroomExperiment`] в поля `VideoHeadroomConfig`.
//!
//! Отдельный модуль (как `viewfinder_variant` для видоискателя): каталог не знает GPU, а настройки не знают каталога.

use isp_vulkan::quality::{
    DemosaicProbe, DemosaicVariant, HeadroomTuning, OutputStageSkips, OutputToneModel, StabilizerMock,
    VideoHeadroomConfig, WorkgroupSizeConfig,
};

use super::catalog::WorkloadKind;
use super::headroom_tuning::{DemosaicChoice, HeadroomExperiment, OutputSkipChoice, StabilizerChoice, ToneModelChoice};

/// Применяет опыт к продуктовым настройкам цепочки V4 (то, чего опыт не задаёт, остаётся как есть).
pub fn apply(experiment: HeadroomExperiment, config: &mut VideoHeadroomConfig) {
    if let Some(shape) = experiment.workgroup {
        config.workgroup_size = WorkgroupSizeConfig { x: shape.x, y: shape.y };
    }
    config.tuning = HeadroomTuning {
        demosaic: demosaic_variant(experiment.demosaic),
        output_skips: output_skips(experiment.output_skips),
        output_tone_model: match experiment.tone_model {
            ToneModelChoice::Dynamic => OutputToneModel::Dynamic,
            ToneModelChoice::AgxOnly => OutputToneModel::AgxOnly,
        },
        stabilizer_mock: match experiment.stabilizer {
            StabilizerChoice::Off => StabilizerMock::Off,
            StabilizerChoice::MergedWithOutput => StabilizerMock::MergedWithOutput,
        },
    };
}

/// Метки проходов работы стенда V4 (продуктовой или с опытом): тесты сверяют по ним строки отчёта.
pub fn pass_labels(kind: WorkloadKind) -> Vec<&'static str> {
    let experiment = match kind {
        WorkloadKind::HeadroomV4Tuned(experiment) => experiment,
        _ => HeadroomExperiment::PRODUCT,
    };
    let mut config = isp_vulkan::saturation::HeadroomSaturationConfig::default();
    apply(experiment, &mut config.headroom);
    config.pass_labels()
}

fn demosaic_variant(choice: DemosaicChoice) -> DemosaicVariant {
    match choice {
        DemosaicChoice::Real => DemosaicVariant::Reference,
        DemosaicChoice::ProbeEmpty => DemosaicVariant::Probe(DemosaicProbe::Empty),
        DemosaicChoice::ProbeCopyF32 => DemosaicVariant::Probe(DemosaicProbe::CopyF32),
        DemosaicChoice::ProbeCopyHalf => DemosaicVariant::Probe(DemosaicProbe::CopyHalf),
    }
}

fn output_skips(choice: OutputSkipChoice) -> OutputStageSkips {
    OutputStageSkips {
        camera_matrix: choice.camera_matrix,
        tone_base: choice.tone_base,
        look: choice.look,
        output_gamut: choice.output_gamut,
        oetf: choice.oetf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_experiment_leaves_the_product_config_untouched() {
        let mut config = VideoHeadroomConfig::default();
        apply(HeadroomExperiment::PRODUCT, &mut config);
        assert_eq!(config, VideoHeadroomConfig::default());
    }

    #[test]
    fn experiment_fields_reach_the_config() {
        let mut config = VideoHeadroomConfig::default();
        let experiment = HeadroomExperiment::PRODUCT
            .workgroup(64, 4)
            .demosaic(DemosaicChoice::ProbeCopyHalf)
            .skip_output(OutputSkipChoice {
                oetf: true,
                ..OutputSkipChoice::NONE
            });
        apply(experiment, &mut config);
        assert_eq!(config.workgroup_size, WorkgroupSizeConfig { x: 64, y: 4 });
        assert_eq!(config.tuning.demosaic, DemosaicVariant::Probe(DemosaicProbe::CopyHalf));
        assert!(config.tuning.output_skips.oetf && !config.tuning.output_skips.look);
    }
}
