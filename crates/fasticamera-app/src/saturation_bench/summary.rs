//! Сводка результата стенда насыщения: компактная таблица строки результата и подробности по работам.
//!
//! Строка `FASTICAMERA_RESULT` — одна запись logcat (~4000 байт), поэтому в ней одна КОРОТКАЯ строка на работу
//! (`имя|к/с|мс/кадр|вердикт`), а не одна длинная строка таблицы (длинные строки сводки укорачиваются до 160 байт).
//! Причины провалов, проходы, шкала, тепло и состав сцены уходят подробностями (`FASTICAMERA_DETAIL`) по работе.

use serde::Serialize;
use serde_json::{Value, json};

use super::catalog::{CatalogEntry, WorkloadKind, find_entry};
use super::digest_report::{DigestOutcome, NO_DIGEST_CELL};
use super::inputs::{LookSummary, SensorSource};
use super::parity_report::ParityOutcome;
use super::report::{RenderSilence, RunSummary, WorkloadRow, thermal_word};
use crate::gpu_services::ValidationState;
use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
use crate::result_line::RESULT_LINE_PREFIX;

/// Что стало с камерой на время замера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraSilence {
    /// Сценарий закрыл камеру и дождался `Closed` (она вернётся по концу сценария).
    ClosedByScenario,
    /// Движка камеры нет: стримить было нечему.
    EngineUnavailable,
}

/// Общие факты прогона для сводки.
#[derive(Debug, Clone)]
pub struct SummaryHeader {
    pub validation: ValidationState,
    pub sensor: SensorSource,
    pub look: LookSummary,
    pub camera: CameraSilence,
    /// Запрошенные работы в порядке списка. В сводку идут не сами имена (они уже есть в таблице, по строке на работу в
    /// том же порядке), а их число и имена, которым не досталось строки таблицы: скрипт ПК сверяет одно с другим.
    pub requested: Vec<&'static str>,
    pub warmup_ms: u64,
    pub measure_ms: u64,
    pub frames_in_flight: usize,
    /// Допуск продуктовой работы GPU потока рендера за замер, кадров (из конфига): по нему скрипт ПК перепроверяет вердикт.
    pub render_tolerance_frames: u64,
}

/// Число с тремя значащими цифрами: `123`, `45.6`, `7.89`, `0.123`.
pub fn format_sig3(value: f64) -> String {
    if !value.is_finite() || value == 0.0 {
        return format!("{value}");
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).clamp(0, 12) as usize;
    format!("{value:.decimals$}")
}

/// Строка компактной таблицы: `имя|к/с|мс/кадр|вердикт`.
pub fn table_row(row: &WorkloadRow) -> String {
    let (fps, cost) = match row.measurement() {
        Some(measurement) => (
            measurement
                .frames_per_second
                .map_or_else(|| "-".to_owned(), format_sig3),
            measurement.frame_cost_ms.map_or_else(|| "-".to_owned(), format_sig3),
        ),
        None => ("-".to_owned(), "-".to_owned()),
    };
    format!("{}|{}|{}|{}", row.name, fps, cost, row.verdict_word())
}

/// Строка дайджестов: `имя|дайджест` (16 hex-цифр общего отпечатка выхода или `-`, если его нет). Части
/// отпечатка и заметка о кадре — в `FASTICAMERA_DETAIL`. Таблица отдельно от дайджестов: её формат не менялся,
/// и старые сохранённые результаты читаются теми же разборщиками.
pub fn digest_row(row: &WorkloadRow) -> String {
    let digest = row.digest.as_ref().map_or(NO_DIGEST_CELL, DigestOutcome::compact);
    format!("{}|{}", row.name, digest)
}

/// Наибольший размер сводки (JSON), при котором в неё ещё кладётся список дайджестов, байт. Строка результата —
/// одна запись logcat (~4000 байт) вместе с обёрткой сценария; сверх бюджета дайджесты остаются только в деталях
/// (`digests_in_detail_only: true`).
pub const SUMMARY_BUDGET_WITH_DIGESTS_BYTES: usize = 3_000;

/// Сводка для строки результата (укладывается в бюджет по построению: строка на работу короткая).
pub fn build_summary(header: &SummaryHeader, run: &RunSummary, silence: &RenderSilence) -> Value {
    let mut summary = summary_without_digests(header, run, silence);
    let digests: Vec<String> = run.rows.iter().map(digest_row).collect();
    let digests_bytes = serde_json::to_string(&digests).map_or(usize::MAX, |text| text.len());
    let current_bytes = serde_json::to_string(&summary).map_or(usize::MAX, |text| text.len());
    if let Value::Object(map) = &mut summary {
        if current_bytes.saturating_add(digests_bytes) <= SUMMARY_BUDGET_WITH_DIGESTS_BYTES {
            map.insert("digests".to_owned(), json!(digests));
        } else {
            map.insert("digests_in_detail_only".to_owned(), json!(true));
        }
    }
    summary
}

/// Ячейка компактной строки, когда числа нет.
const NO_VALUE_CELL: &str = "-";

/// Работа паритета на устройстве.
fn is_parity_kind(kind: WorkloadKind) -> bool {
    matches!(kind, WorkloadKind::LookParity { .. })
}

/// Работа цветового прибора (любая: настоящая или с урезанным накоплением).
fn is_scope_kind(kind: WorkloadKind) -> bool {
    matches!(kind, WorkloadKind::Scopes { .. } | WorkloadKind::ScopesAblated { .. })
}

/// Сколько из запрошенных работ имеют вид `wanted` (по каталогу).
fn requested_count(header: &SummaryHeader, wanted: fn(WorkloadKind) -> bool) -> usize {
    header
        .requested
        .iter()
        .filter(|name| find_entry(name).is_ok_and(|entry| wanted(entry.kind)))
        .count()
}

/// Строки паритета для результата: ровно по одной на КАЖДУЮ работу паритета в прогоне (по виду работы в каталоге, а не
/// по тому, дошла ли она до числа): пропавшее измерение — строка `…|unavailable`, а не отсутствие строки. Формат —
/// [`ParityOutcome::compact_row`].
pub fn parity_rows(run: &RunSummary) -> Vec<String> {
    run.rows
        .iter()
        .filter(|row| find_entry(row.name).is_ok_and(|entry| is_parity_kind(entry.kind)))
        .map(|row| match &row.extras.parity {
            Some(outcome) => outcome.compact_row(row.name),
            None => ParityOutcome::Unavailable {
                reason: "работа не дошла до измерения".to_owned(),
            }
            .compact_row(row.name),
        })
        .collect()
}

/// Строки «какой размер картинки прибора» для результата: по одной на каждую работу `scopes_*` прогона, `имя|ШxВ`
/// (или `имя|-`, если работа не дошла до размера). Отчёт называет настоящий размер раздела, а не умолчание.
pub fn scope_picture_rows(run: &RunSummary) -> Vec<String> {
    run.rows
        .iter()
        .filter(|row| find_entry(row.name).is_ok_and(|entry| is_scope_kind(entry.kind)))
        .map(|row| {
            let size = row
                .extras
                .scope_picture
                .map_or_else(|| NO_VALUE_CELL.to_owned(), |picture| picture.compact());
            format!("{}|{size}", row.name)
        })
        .collect()
}

fn summary_without_digests(header: &SummaryHeader, run: &RunSummary, silence: &RenderSilence) -> Value {
    let mut summary = base_summary(header, run, silence);
    if let Value::Object(map) = &mut summary {
        // Сколько строк каждого вида ОБЯЗАНО быть по списку запрошенных работ (каталог говорит, какая работа какого
        // вида): скрипт ПК сверяет с этим числом число пришедших строк, и пропавшее измерение не сходит за успех.
        let parity_expected = requested_count(header, is_parity_kind);
        if parity_expected > 0 {
            map.insert("parity_expected".to_owned(), json!(parity_expected));
            map.insert("parity".to_owned(), json!(parity_rows(run)));
        }
        let pictures_expected = requested_count(header, is_scope_kind);
        if pictures_expected > 0 {
            map.insert("scope_pictures_expected".to_owned(), json!(pictures_expected));
            map.insert("scope_pictures".to_owned(), json!(scope_picture_rows(run)));
        }
    }
    summary
}

/// Запрошенные работы, которым не досталось строки таблицы (по построению пусто: строка есть у каждой работы, даже не
/// дошедшей до числа). Непустой список — дефект прогона, и скрипт ПК называет эти работы по именам.
fn requested_without_table_row(header: &SummaryHeader, run: &RunSummary) -> Vec<&'static str> {
    header
        .requested
        .iter()
        .copied()
        .filter(|name| !run.rows.iter().any(|row| row.name == *name))
        .collect()
}

fn base_summary(header: &SummaryHeader, run: &RunSummary, silence: &RenderSilence) -> Value {
    json!({
        "table": run.rows.iter().map(table_row).collect::<Vec<_>>(),
        // Решение 14o: полный список имён дублировал таблицу (≈ 900 байт на 40 работ) и переполнял строку результата.
        "requested_count": header.requested.len(),
        "requested_missing": requested_without_table_row(header, run),
        "workloads": run.rows.len(),
        "measured": run.rows.iter().filter(|row| row.is_measured()).count(),
        "cancelled": run.cancelled,
        "validation": header.validation,
        "sensor": header.sensor,
        "look": header.look,
        "camera": header.camera,
        "warmup_ms": header.warmup_ms,
        "measure_ms": header.measure_ms,
        "frames_in_flight": header.frames_in_flight,
        "render_tolerance_frames": header.render_tolerance_frames,
        "render": silence,
    })
}

/// Полная строка одной работы для `FASTICAMERA_DETAIL`.
pub fn detail_item(row: &WorkloadRow) -> Value {
    let mut item =
        serde_json::to_value(row).unwrap_or_else(|error| json!({ "serialization_error": error.to_string() }));
    if let Value::Object(map) = &mut item
        && let (Some(start), Some(end)) = (&row.thermal_start, &row.thermal_end)
    {
        map.insert(
            "thermal_words".to_owned(),
            json!({ "start": thermal_word(start), "end": thermal_word(end) }),
        );
    }
    item
}

/// Сколько байт строки результата оставлено на текст ошибки провала: столько же, сколько отдаёт ему
/// `scenario::camera::result_budget::MAX_ERROR_BYTES` (равенство проверяет тест сценария стенда).
pub const ERROR_TEXT_RESERVE_BYTES: usize = 700;
/// Обёртка отчёта вокруг сводки: имя сценария, статус, ключи и кавычки, байт (с запасом).
const REPORT_ENVELOPE_RESERVE_BYTES: usize = 120;
/// Шапка сводки без строк по работам (сенсор, вид, камера, окна, счётчики рендера), байт. Число — сверху: тест
/// `worst_case_header_fits_its_reserve` собирает шапку с самыми длинными значениями и проверяет, что она не больше.
pub const SUMMARY_HEADER_RESERVE_BYTES: usize = 1_050;
/// Самая длинная ячейка числа в строке таблицы (`к/с` или `мс/кадр`, три значащие цифры: `65800` / `0.123`).
const WIDEST_NUMBER_CELL_BYTES: usize = 6;
/// Самое длинное слово вердикта строки таблицы (`slots_mismatch` и подобные — короче).
const WIDEST_VERDICT_BYTES: usize = 16;
/// Хвост строки паритета после имени: `|1.35e-5|5.02e-4|1e-5|unavailable` и запас на кавычки и запятую.
const PARITY_ROW_TAIL_BYTES: usize = 40;
/// Хвост строки размера прибора после имени: `|2048x2048` и запас на кавычки и запятую.
const SCOPE_PICTURE_ROW_TAIL_BYTES: usize = 14;

/// Сколько байт может занять сводка (без дайджестов, они кладутся только при запасе) для списка работ `entries`:
/// оценка СВЕРХУ по самым длинным ячейкам. Нужна до запуска: стенд, чей отчёт не влезет в строку результата, отказывает
/// при `initialize`, а не через десять минут замера (как было на телефоне в 14o: 40 работ — отчёт 4532 байт).
pub fn worst_case_summary_bytes(entries: &[&CatalogEntry]) -> usize {
    let rows: usize = entries
        .iter()
        .map(|entry| {
            let name = entry.name.len();
            // Строка таблицы: имя|к/с|мс/кадр|вердикт + кавычки и запятая.
            let table = name + 3 + 2 * WIDEST_NUMBER_CELL_BYTES + WIDEST_VERDICT_BYTES + 3;
            let parity = if is_parity_kind(entry.kind) {
                name + PARITY_ROW_TAIL_BYTES
            } else {
                0
            };
            let picture = if is_scope_kind(entry.kind) {
                name + SCOPE_PICTURE_ROW_TAIL_BYTES
            } else {
                0
            };
            table + parity + picture
        })
        .sum();
    SUMMARY_HEADER_RESERVE_BYTES + rows
}

/// Наибольший размер сводки, при котором отчёт целиком (с текстом ошибки провала) влезает в одну строку logcat.
pub const fn summary_budget_bytes() -> usize {
    LOGCAT_MAX_TEXT_BYTES - RESULT_LINE_PREFIX.len() - ERROR_TEXT_RESERVE_BYTES - REPORT_ENVELOPE_RESERVE_BYTES
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
