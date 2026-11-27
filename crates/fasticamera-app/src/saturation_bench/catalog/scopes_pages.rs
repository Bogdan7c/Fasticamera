//! Работы группы `scopes_pages`: цена страниц приборов сверх `baseline` (14r, решение 321; 14s — гистограмма в трёх
//! режимах и CIE в двух шкалах). В `baseline` они не кладутся: состав и порядок умолчания неизменны, а строка отчёта ограничена.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "scopes_luma_waveform",
        kind: scopes(ScopeShape::LumaWaveform, ScopeInputKind::Natural),
        group: WorkloadGroup::ScopesPages,
        summary: "прибор LUMA WAVEFORM на сцене",
    },
    CatalogEntry {
        name: "scopes_histogram_rgb",
        kind: scopes(ScopeShape::HistogramRgb, ScopeInputKind::Natural),
        group: WorkloadGroup::ScopesPages,
        summary: "гистограмма, режим RGB (три плоскости, столбики наложением) на сцене",
    },
    CatalogEntry {
        name: "scopes_histogram_parade",
        kind: scopes(ScopeShape::HistogramParade, ScopeInputKind::Natural),
        group: WorkloadGroup::ScopesPages,
        summary: "гистограмма, режим PARADE (те же счётчики, столбики секциями) на сцене",
    },
    CatalogEntry {
        name: "scopes_histogram_luma",
        kind: scopes(ScopeShape::HistogramLuma, ScopeInputKind::Natural),
        group: WorkloadGroup::ScopesPages,
        summary: "гистограмма, режим Y (одна плоскость яркости) на сцене",
    },
    CatalogEntry {
        name: "scopes_cie_xy",
        kind: scopes(ScopeShape::CieXy, ScopeInputKind::Natural),
        group: WorkloadGroup::ScopesPages,
        summary: "диаграмма CIE, шкала xy 1931 (одна плоскость, точное деление в ячейку) на сцене",
    },
    CatalogEntry {
        name: "scopes_cie_uv",
        kind: scopes(ScopeShape::CieUv, ScopeInputKind::Natural),
        group: WorkloadGroup::ScopesPages,
        summary: "диаграмма CIE, шкала u′v′ 1976 (те же проходы, другие числа правила) на сцене",
    },
];
