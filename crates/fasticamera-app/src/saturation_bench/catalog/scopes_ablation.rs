//! Работы группы `scopes_ablation`: разложение цены накопления приборов (чтение, проекция, запись).
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "scopes_waveform_empty",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Empty),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: пустое накопление (очистка + запуск групп)",
    },
    CatalogEntry {
        name: "scopes_waveform_read_only",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::ReadOnly),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: только чтение и квантование",
    },
    CatalogEntry {
        name: "scopes_waveform_skip_write",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::SkipWrite),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: чтение + проекция, без записи",
    },
    CatalogEntry {
        name: "scopes_waveform_cell_zero",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::WriteCellZero),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: все atomicAdd в одну ячейку",
    },
    CatalogEntry {
        name: "scopes_waveform_spread",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::WriteSpread),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: у каждой нити своя ячейка",
    },
    CatalogEntry {
        name: "scopes_parade_empty",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Empty),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: пустое накопление (очистка + запуск групп)",
    },
    CatalogEntry {
        name: "scopes_parade_read_only",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::ReadOnly),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: только чтение и квантование",
    },
    CatalogEntry {
        name: "scopes_parade_skip_write",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::SkipWrite),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: чтение + проекция, без записи",
    },
    CatalogEntry {
        name: "scopes_parade_cell_zero",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::WriteCellZero),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: все atomicAdd в одну ячейку",
    },
    CatalogEntry {
        name: "scopes_parade_spread",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::WriteSpread),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: у каждой нити своя ячейка",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_empty",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::Empty),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: пустое накопление (очистка + запуск групп)",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_read_only",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::ReadOnly),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: только чтение и квантование",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_skip_write",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::SkipWrite),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: чтение + проекция, без записи",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_cell_zero",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::WriteCellZero),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: все atomicAdd в одну ячейку",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_spread",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::WriteSpread),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: у каждой нити своя ячейка",
    },
    CatalogEntry {
        name: "scopes_waveform_aggregated",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_parade_aggregated",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr_aggregated",
        kind: scopes_ablated(ScopeShape::VectorscopeYcbcr, ScopeAblation::Aggregated),
        group: WorkloadGroup::ScopesAblation,
        summary: "vectorscope_ycbcr: слияние одинаковых ячеек в subgroup",
    },
    CatalogEntry {
        name: "scopes_waveform_runs4",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs4),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: 4 отсчёта на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_waveform_runs8",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs8),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: 8 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs4",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs4),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: 4 отсчёта на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs8",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs8),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: 8 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_waveform_runs16",
        kind: scopes_ablated(ScopeShape::Waveform, ScopeAblation::Runs16),
        group: WorkloadGroup::ScopesAblation,
        summary: "waveform: 16 отсчётов на нить, серии в регистре",
    },
    CatalogEntry {
        name: "scopes_parade_runs16",
        kind: scopes_ablated(ScopeShape::Parade, ScopeAblation::Runs16),
        group: WorkloadGroup::ScopesAblation,
        summary: "parade: 16 отсчётов на нить, серии в регистре",
    },
];
