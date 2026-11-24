//! Работы группы `still_tuning`: опыты над проявлением снимка (высота полосы, форма рабочей группы).
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строка с опытной настройкой проявления.
const fn entry(name: &'static str, tuning: StillTuning, summary: &'static str) -> CatalogEntry {
    CatalogEntry {
        name,
        kind: WorkloadKind::StillTuned { tuning },
        group: WorkloadGroup::StillTuning,
        summary,
    }
}

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    entry(
        "still_product",
        StillTuning::PRODUCT,
        "проявление снимка как в продукте (полоса 256, группа 16x16)",
    ),
    entry(
        "still_slice_128",
        StillTuning::PRODUCT.slice_rows(128),
        "полоса 128 строк",
    ),
    entry(
        "still_slice_512",
        StillTuning::PRODUCT.slice_rows(512),
        "полоса 512 строк",
    ),
    entry(
        "still_slice_1024",
        StillTuning::PRODUCT.slice_rows(1024),
        "полоса 1024 строки",
    ),
    entry("still_wg_128x2", StillTuning::PRODUCT.workgroup(128, 2), "группа 128x2"),
    entry("still_wg_32x8", StillTuning::PRODUCT.workgroup(32, 8), "группа 32x8"),
    entry("still_wg_64x4", StillTuning::PRODUCT.workgroup(64, 4), "группа 64x4"),
    entry("still_wg_8x8", StillTuning::PRODUCT.workgroup(8, 8), "группа 8x8"),
    entry(
        "still_product_twin",
        StillTuning::PRODUCT,
        "двойник still_product (повтор в A/B/A)",
    ),
];
