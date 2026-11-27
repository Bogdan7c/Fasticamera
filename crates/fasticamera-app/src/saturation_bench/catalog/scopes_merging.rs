//! Работы группы `scopes_merging`: опыты слияния записей накопления приборов (в subgroup — `_aggregated`, сериями в
//! регистре — `_runs4/8/16`). Отделены от `scopes_ablation` в 14r: 24 работы разом не влезали в одну строку отчёта
//! (`TooManyForOneResultLine`, проверка 14o), и группа отвергалась до запуска.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "scopes_waveform_aggregated",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesMerging,
        summary: "waveform: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_parade_aggregated",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesMerging,
        summary: "parade: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_aggregated",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesMerging,
        summary: "vectorscope_ycbcr: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_waveform_runs4",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs4),
        group: WorkloadGroup::ScopesMerging,
        summary: "waveform: 4 отсчёта на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_waveform_runs8",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs8),
        group: WorkloadGroup::ScopesMerging,
        summary: "waveform: 8 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs4",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs4),
        group: WorkloadGroup::ScopesMerging,
        summary: "parade: 4 отсчёта на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs8",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs8),
        group: WorkloadGroup::ScopesMerging,
        summary: "parade: 8 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_waveform_runs16",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs16),
        group: WorkloadGroup::ScopesMerging,
        summary: "waveform: 16 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs16",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs16),
        group: WorkloadGroup::ScopesMerging,
        summary: "parade: 16 отсчётов на нить, серии в регистре",
    },
];
