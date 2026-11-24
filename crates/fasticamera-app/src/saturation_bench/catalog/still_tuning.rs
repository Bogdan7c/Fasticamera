//! Работы группы `still_tuning`: опыты над проявлением снимка (высота полосы, форма рабочей группы).
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use look_contract::ToneBaseModel;

use super::*;
use crate::saturation_bench::still_variant::StillOutputPath;

/// Строка с опытной настройкой проявления.
const fn entry(name: &'static str, tuning: StillTuning, summary: &'static str) -> CatalogEntry {
    CatalogEntry {
        name,
        kind: WorkloadKind::StillTuned { tuning },
        group: WorkloadGroup::StillTuning,
        summary,
    }
}

/// Строка с кадром под моделью основы тона `model` и путём выходного прохода `path`.
const fn tone_entry(
    name: &'static str,
    model: ToneBaseModel,
    path: StillOutputPath,
    summary: &'static str,
) -> CatalogEntry {
    entry(name, StillTuning::PRODUCT.tone_model(model, path), summary)
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
    tone_entry(
        "still_agx_per_model",
        ToneBaseModel::Agx,
        StillOutputPath::PerModel,
        "снимок, основа agx: вариант пайплайна на модель",
    ),
    tone_entry(
        "still_agx_runtime",
        ToneBaseModel::Agx,
        StillOutputPath::Runtime,
        "снимок, основа agx: прежний путь, модель на лету",
    ),
    tone_entry(
        "still_aces_per_model",
        ToneBaseModel::Aces,
        StillOutputPath::PerModel,
        "снимок, основа aces: вариант пайплайна на модель",
    ),
    tone_entry(
        "still_aces_runtime",
        ToneBaseModel::Aces,
        StillOutputPath::Runtime,
        "снимок, основа aces: прежний путь, модель на лету",
    ),
    tone_entry(
        "still_filmic_per_model",
        ToneBaseModel::Filmic,
        StillOutputPath::PerModel,
        "снимок, основа filmic: вариант пайплайна на модель",
    ),
    tone_entry(
        "still_filmic_runtime",
        ToneBaseModel::Filmic,
        StillOutputPath::Runtime,
        "снимок, основа filmic: прежний путь, модель на лету",
    ),
    tone_entry(
        "still_legacy_per_model",
        ToneBaseModel::Legacy,
        StillOutputPath::PerModel,
        "снимок, основа legacy: вариант пайплайна на модель",
    ),
    tone_entry(
        "still_legacy_runtime",
        ToneBaseModel::Legacy,
        StillOutputPath::Runtime,
        "снимок, основа legacy: прежний путь, модель на лету",
    ),
];
