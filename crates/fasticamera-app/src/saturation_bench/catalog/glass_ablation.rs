//! Работы группы `glass_ablation`: что из цены стекла чем занято (пакет G3 сессии 16a). Настоящий список рисования
//! экрана, только стекло; меняется состав групп (`GlassSubset`) или стиль (`GlassStyleAblation`).
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "display_calibrate_glass_no_tone",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::NoToneAndDimming,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без плёнки тона и затемнения (без 5 выборок средней яркости)",
    },
    CatalogEntry {
        name: "display_calibrate_glass_no_refraction",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::NoRefraction,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без преломления (одна выборка фона вместо трёх на краях)",
    },
    CatalogEntry {
        name: "display_calibrate_glass_whole_level",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::WholeBlurLevel,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, целый уровень размытия (одна выборка фона вместо двух)",
    },
    CatalogEntry {
        name: "display_calibrate_glass_no_shadow",
        kind: glass_ablated(DisplayScreen::Calibrate, GlassSubset::All, GlassStyleAblation::NoShadow),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без тени",
    },
    CatalogEntry {
        name: "display_calibrate_glass_no_finishing",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::NoFinishing,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, без блика, тёмного края, свечения и насыщенности",
    },
    CatalogEntry {
        name: "display_calibrate_glass_floor",
        kind: glass_ablated(DisplayScreen::Calibrate, GlassSubset::All, GlassStyleAblation::Floor),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, пол цены: поле формы + одна выборка размытого фона + смешивание",
    },
    CatalogEntry {
        name: "display_calibrate_glass_floor_sharp",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::FloorSharp,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло, пол цены с резким фоном (одна выборка полноразмерной картинки)",
    },
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
    CatalogEntry {
        name: "display_calibrate_glass_panel_only",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::LargestOnly,
            GlassStyleAblation::Product,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только самая большая группа стекла (панель ручек)",
    },
    CatalogEntry {
        name: "display_calibrate_glass_no_panel",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::WithoutLargest,
            GlassStyleAblation::Product,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло без самой большой группы",
    },
    CatalogEntry {
        name: "display_calibrate_glass_invisible",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::Invisible,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло с прозрачностью 0: растеризация, два поля формы, смешивание, без выборок фона",
    },
    CatalogEntry {
        name: "display_calibrate_glass_none",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::NoGroups,
            GlassStyleAblation::Product,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "стекла нет вовсе: цена всего, что не зависит от групп (компоновщик, пирамида, проход)",
    },
    CatalogEntry {
        name: "display_calibrate_glass_invisible_no_shadow",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::All,
            GlassStyleAblation::InvisibleNoShadow,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только стекло с прозрачностью 0 и без тени: растеризация, одно поле формы, смешивание",
    },
    CatalogEntry {
        name: "display_calibrate_glass_smallest_invisible",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::SmallestOnly,
            GlassStyleAblation::Invisible,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "одна самая маленькая группа стекла, прозрачность 0: постоянная цена прохода и пирамиды",
    },
    CatalogEntry {
        name: "display_calibrate_glass_panel_invisible",
        kind: glass_ablated(
            DisplayScreen::Calibrate,
            GlassSubset::LargestOnly,
            GlassStyleAblation::Invisible,
        ),
        group: WorkloadGroup::GlassAblation,
        summary: "только панель ручек, прозрачность 0",
    },
];
