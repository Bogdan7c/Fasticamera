//! Работы группы `scopes_pages`: цена страниц приборов сверх `baseline` (14r, решение 321; в 14s сюда лягут
//! гистограмма и CIE). В `baseline` они не кладутся: состав и порядок умолчания неизменны, а строка отчёта ограничена.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[CatalogEntry {
    name: "scopes_luma_waveform",
    kind: scopes(ScopeShape::LumaWaveform, ScopeInputKind::Natural),
    group: WorkloadGroup::ScopesPages,
    summary: "прибор LUMA WAVEFORM на сцене",
}];
