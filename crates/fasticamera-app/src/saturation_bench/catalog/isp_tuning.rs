//! Работы группы `isp_tuning`: опыты над слитым проходом видоискателя (форма группы, чтение RAW, сведение AWB).
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "viewfinder_awb_every_frame_shared",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .shared_memory_awb(),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, сведение только через shared-окно",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_8x8",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(8, 8)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 8x8",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_16x16",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(16, 16)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 16x16",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_32x8",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(32, 8)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 32x8",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_32x16",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(32, 16)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 32x16",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_64x4",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(64, 4)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 64x4",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_4x32",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(4, 32)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 4x32",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_8x16",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(8, 16)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 8x16",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_2x64",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(2, 64)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 2x64",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_4x64",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(4, 64)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 4x64",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_8x32",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(8, 32)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 8x32",
    },
    CatalogEntry {
        name: "viewfinder_never_wg_1x128",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).workgroup(1, 128)),
        group: WorkloadGroup::IspTuning,
        summary: "без AWB, группа 1x128",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_4x32",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(4, 32),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 4x32",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_8x16",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(8, 16),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 8x16",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_16x16",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(16, 16),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 16x16",
    },
    CatalogEntry {
        name: "viewfinder_twin",
        kind: tuned(ViewfinderTuning::PRODUCT),
        group: WorkloadGroup::IspTuning,
        summary: "двойник работы viewfinder (повтор в A/B/A: имена в списке не повторяются)",
    },
    CatalogEntry {
        name: "viewfinder_awb_never_twin",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never)),
        group: WorkloadGroup::IspTuning,
        summary: "двойник viewfinder_awb_never",
    },
    CatalogEntry {
        name: "viewfinder_awb_every_frame_twin",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::EveryFrame)),
        group: WorkloadGroup::IspTuning,
        summary: "двойник viewfinder_awb_every_frame",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_32x8",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(32, 8),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 32x8",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_64x4",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(64, 4),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 64x4",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_8x32",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(8, 32),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 8x32",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_4x64",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(4, 64),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 4x64",
    },
    CatalogEntry {
        name: "viewfinder_every_frame_wg_32x16",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(32, 16),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр, группа 32x16",
    },
    CatalogEntry {
        name: "viewfinder_minimal_twin",
        kind: tuned(ViewfinderTuning::PRODUCT.minimal()),
        group: WorkloadGroup::IspTuning,
        summary: "двойник viewfinder_minimal (уровни + демозаик)",
    },
    CatalogEntry {
        name: "viewfinder_minimal_wg_4x32",
        kind: tuned(ViewfinderTuning::PRODUCT.minimal().workgroup(4, 32)),
        group: WorkloadGroup::IspTuning,
        summary: "минимум, группа 4x32",
    },
    CatalogEntry {
        name: "viewfinder_minimal_wg_8x16",
        kind: tuned(ViewfinderTuning::PRODUCT.minimal().workgroup(8, 16)),
        group: WorkloadGroup::IspTuning,
        summary: "минимум, группа 8x16",
    },
    CatalogEntry {
        name: "viewfinder_legacy_subgroup",
        kind: tuned(ViewfinderTuning::PRODUCT.subgroup_awb()),
        group: WorkloadGroup::IspTuning,
        summary: "продукт до пакета I: AWB через subgroup, каждый 4-й кадр",
    },
    CatalogEntry {
        name: "viewfinder_awb_every_frame_legacy_subgroup",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .subgroup_awb(),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр через subgroup (прежний продукт)",
    },
    CatalogEntry {
        name: "viewfinder_wg_16x8",
        kind: tuned(ViewfinderTuning::PRODUCT.workgroup(16, 8)),
        group: WorkloadGroup::IspTuning,
        summary: "продукт с прежней группой 16x8 (A/B с новой группой 4x32)",
    },
    CatalogEntry {
        name: "viewfinder_awb_every_frame_wg_16x8",
        kind: tuned(
            ViewfinderTuning::PRODUCT
                .schedule(AwbSchedule::EveryFrame)
                .workgroup(16, 8),
        ),
        group: WorkloadGroup::IspTuning,
        summary: "AWB каждый кадр с прежней группой 16x8",
    },
    // ---- Диагностика «ISP в размере экрана»: цена прохода почти пропорциональна числу пикселей выхода. ----
    CatalogEntry {
        name: "viewfinder_zoom_1420_never",
        kind: tuned(ViewfinderTuning::PRODUCT.schedule(AwbSchedule::Never).crop_zoom(1420)),
        group: WorkloadGroup::IspTuning,
        summary: "ДИАГНОСТИКА: выход вдвое меньше по числу пикселей (кроп ×1,42), без AWB",
    },
    CatalogEntry {
        name: "viewfinder_zoom_1420",
        kind: tuned(ViewfinderTuning::PRODUCT.crop_zoom(1420)),
        group: WorkloadGroup::IspTuning,
        summary: "ДИАГНОСТИКА: выход вдвое меньше по числу пикселей (кроп ×1,42), AWB как в продукте",
    },
];
