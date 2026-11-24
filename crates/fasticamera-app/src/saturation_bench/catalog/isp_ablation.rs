//! Работы группы `isp_ablation`: слитый проход видоискателя без отдельных стадий и с разной каденцией AWB.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "viewfinder_awb_never",
        kind: viewfinder(ViewfinderVariant::AwbNever),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя: статистика AWB не записывается никогда",
    },
    CatalogEntry {
        name: "viewfinder_awb_every_frame",
        kind: viewfinder(ViewfinderVariant::AwbEveryFrame),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя: статистика AWB на каждом кадре",
    },
    CatalogEntry {
        name: "viewfinder_without_levels",
        kind: without(AblatedStage::Levels),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии levels",
    },
    CatalogEntry {
        name: "viewfinder_without_lens_shading",
        kind: without(AblatedStage::LensShading),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии lens_shading",
    },
    CatalogEntry {
        name: "viewfinder_without_white_balance",
        kind: without(AblatedStage::WhiteBalance),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии white_balance",
    },
    CatalogEntry {
        name: "viewfinder_without_demosaic",
        kind: without(AblatedStage::Demosaic),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии demosaic (читается один отсчёт ячейки вместо четырёх, пока включена статистика AWB — всё равно четыре)",
    },
    CatalogEntry {
        name: "viewfinder_without_color",
        kind: without(AblatedStage::Color),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии color",
    },
    CatalogEntry {
        name: "viewfinder_without_gamut_compression",
        kind: without(AblatedStage::GamutCompression),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии gamut_compression",
    },
    CatalogEntry {
        name: "viewfinder_without_tone",
        kind: without(AblatedStage::Tone),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии tone",
    },
    CatalogEntry {
        name: "viewfinder_without_look_color",
        kind: without(AblatedStage::LookColor),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии look_color",
    },
    CatalogEntry {
        name: "viewfinder_without_look_lut",
        kind: without(AblatedStage::LookLut),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии look_lut (в продукте она и так вырезается кадром без таблицы вида)",
    },
    CatalogEntry {
        name: "viewfinder_without_output_gamut_compression",
        kind: without(AblatedStage::OutputGamutCompression),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии output_gamut_compression (у прежней основы тона её в проходе нет)",
    },
    CatalogEntry {
        name: "viewfinder_without_oetf",
        kind: without(AblatedStage::Oetf),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя без стадии oetf",
    },
    CatalogEntry {
        name: "viewfinder_minimal",
        kind: viewfinder(ViewfinderVariant::Minimal),
        group: WorkloadGroup::IspAblation,
        summary: "ISP видоискателя, пол цены: только levels + demosaic (4 отсчёта RAW на пиксель, запись картинки)",
    },
];
