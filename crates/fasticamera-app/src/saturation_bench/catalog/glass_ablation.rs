//! Работы группы `glass_ablation`: что из цены стекла чем занято (пакет G3 сессии 16a). Настоящий список рисования
//! экрана PHOTO, только стекло; меняется состав групп (`GlassSubset`) или стиль (`GlassStyleAblation`).
//!
//! После 14m в разделе CALIBRATE стекла нет (кроме редкой вуали перехода с удержанным кадром), поэтому работ
//! по стеклу CALIBRATE здесь нет: они мерили бы пустоту.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "display_photo_glass_no_tone",
        kind: glass_ablated(
            DisplayScreen::Photo,
            GlassSubset::All,
            GlassStyleAblation::NoToneAndDimming,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без плёнки тона и затемнения (без 5 выборок средней яркости)",
    },
    CatalogEntry {
        name: "display_photo_glass_no_refraction",
        kind: glass_ablated(DisplayScreen::Photo, GlassSubset::All, GlassStyleAblation::NoRefraction),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без преломления (одна выборка фона вместо трёх на краях)",
    },
    CatalogEntry {
        name: "display_photo_glass_whole_level",
        kind: glass_ablated(
            DisplayScreen::Photo,
            GlassSubset::All,
            GlassStyleAblation::WholeBlurLevel,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, целый уровень размытия (одна выборка фона вместо двух)",
    },
    CatalogEntry {
        name: "display_photo_glass_no_shadow",
        kind: glass_ablated(DisplayScreen::Photo, GlassSubset::All, GlassStyleAblation::NoShadow),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без тени",
    },
    CatalogEntry {
        name: "display_photo_glass_no_finishing",
        kind: glass_ablated(DisplayScreen::Photo, GlassSubset::All, GlassStyleAblation::NoFinishing),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без блика, тёмного края, свечения и насыщенности",
    },
    CatalogEntry {
        name: "display_photo_glass_floor",
        kind: glass_ablated(DisplayScreen::Photo, GlassSubset::All, GlassStyleAblation::Floor),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, пол цены: поле формы + одна выборка размытого фона + смешивание",
    },
    CatalogEntry {
        name: "display_photo_glass_floor_sharp",
        kind: glass_ablated(DisplayScreen::Photo, GlassSubset::All, GlassStyleAblation::FloorSharp),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, пол цены с резким фоном (одна выборка полноразмерной картинки)",
    },
];
