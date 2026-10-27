//! Бюджет строки результата для провальных прогонов сценариев видоискателя (сессия 07).
//!
//! Строка `FASTICAMERA_RESULT` — одна запись logcat (4000 байт, см. `detail.rs`). Удачный прогон
//! укладывается в неё по построению, а провальный раньше мог раздуться: список нарушений растёт с
//! числом окон/строк, а свободный текст причин (отказ стенда, ошибка параметров ISP, ответ движка)
//! ничем не ограничен. Перебор бюджета заменяет весь отчёт сообщением «не помещается» — вердикт и
//! числа, ради которых прогон и запускался, теряются именно тогда, когда они нужнее всего.
//!
//! Правило модуля: в строку результата идут только ограниченные части.
//! * Текст ошибки — первые нарушения целиком (каждое не длиннее [`MAX_VIOLATION_BYTES`]), пока
//!   влезают в [`MAX_ERROR_BYTES`], затем «…ещё K»; полный список — строками `detail_violations`.
//! * Любая строка в сводке укорачивается до [`MAX_SUMMARY_TEXT_BYTES`] (по границе символа —
//!   причины у нас на кириллице, два байта на букву).
//! * Неограниченные списки сводки (недоставленные регулировки) — через [`bounded_list`]:
//!   число + первые [`MAX_LIST_ITEMS_IN_SUMMARY`].

use serde_json::Value;

use super::detail::emit_details;
use crate::scenario::ScenarioOutcome;
use crate::scenario::observing::{ScenarioContext, judged};

/// Наибольшая длина одного нарушения в тексте ошибки, байт.
pub const MAX_VIOLATION_BYTES: usize = 200;
/// Наибольшая длина текста ошибки целиком, байт: остальное место строки — сводке.
pub const MAX_ERROR_BYTES: usize = 700;
/// Наибольшая длина любой строки внутри сводки, байт (таблица этапов, причины, ошибки датчиков).
pub const MAX_SUMMARY_TEXT_BYTES: usize = 160;
/// Сколько элементов неограниченного списка показывать в сводке.
pub const MAX_LIST_ITEMS_IN_SUMMARY: usize = 3;
/// Чем помечается укороченный текст.
const ELLIPSIS: &str = "…";

/// Итог сценария, уложенный в бюджет строки результата.
#[derive(Debug)]
pub struct BudgetedVerdict {
    pub outcome: ScenarioOutcome,
    /// Полный список нарушений для `detail_violations`; пустой — в тексте ошибки ничего не потеряно.
    pub violation_details: Vec<Value>,
}

/// Вердикт по сводке и нарушениям в пределах бюджета строки (чистая функция, см. модуль).
pub fn judge_within_budget(mut summary: Value, violations: Vec<String>) -> BudgetedVerdict {
    bound_strings(&mut summary, MAX_SUMMARY_TEXT_BYTES);
    let compact = compact_violations(&violations);
    let violation_details = if compact.lost_text {
        violations.into_iter().map(Value::String).collect()
    } else {
        Vec::new()
    };
    let outcome = match compact.error {
        // Нарушений нет: `judged` даёт `Passed`.
        None => judged(summary, Vec::new()),
        Some(error) => judged(summary, vec![error]),
    };
    BudgetedVerdict {
        outcome,
        violation_details,
    }
}

/// [`judge_within_budget`] + вывод полного списка нарушений строками `detail_violations`, если в
/// тексте ошибки он не уместился. Подробности пишутся ДО результата (как все `detail_*`).
pub fn finish_within_budget(
    context: &mut ScenarioContext<'_>,
    summary: Value,
    violations: Vec<String>,
) -> ScenarioOutcome {
    let verdict = judge_within_budget(summary, violations);
    if !verdict.violation_details.is_empty() {
        emit_details(context, "detail_violations", verdict.violation_details);
    }
    verdict.outcome
}

/// Текст ошибки из нарушений.
struct CompactViolations {
    /// `None` — нарушений нет.
    error: Option<String>,
    /// Что-то из нарушений не попало в текст (укорочено или не влезло).
    lost_text: bool,
}

fn compact_violations(violations: &[String]) -> CompactViolations {
    if violations.is_empty() {
        return CompactViolations {
            error: None,
            lost_text: false,
        };
    }
    let mut error = String::new();
    let mut lost_text = false;
    let mut included = 0_usize;
    for violation in violations {
        let shortened = truncate_text(violation, MAX_VIOLATION_BYTES);
        lost_text |= shortened.len() != violation.len();
        let separator = if error.is_empty() { "" } else { "; " };
        // Место под хвост «…ещё K» оставляем всегда: иначе он сам вылез бы за бюджет.
        if error.len() + separator.len() + shortened.len() + TAIL_RESERVE_BYTES > MAX_ERROR_BYTES {
            break;
        }
        error.push_str(separator);
        error.push_str(&shortened);
        included += 1;
    }
    let skipped = violations.len() - included;
    if skipped > 0 {
        lost_text = true;
        let separator = if error.is_empty() { "" } else { "; " };
        error.push_str(&format!(
            "{separator}…ещё {skipped} (полный список — detail_violations)"
        ));
    }
    CompactViolations {
        error: Some(error),
        lost_text,
    }
}

/// Сколько байт держать под хвост «; …ещё K (полный список — detail_violations)» (~70 байт с кириллицей).
const TAIL_RESERVE_BYTES: usize = 80;

/// Укорачивает текст до `max_bytes` байт по границе символа UTF-8, помечая обрезку многоточием.
pub fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let keep = max_bytes.saturating_sub(ELLIPSIS.len());
    // Ближайшая граница символа не дальше `keep`: срез посреди кириллической буквы — паника.
    let boundary = (0..=keep)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0);
    format!("{}{ELLIPSIS}", &text[..boundary])
}

/// Список неизвестной длины для сводки: число + первые элементы (укороченные).
pub fn bounded_list(items: &[String]) -> Value {
    serde_json::json!({
        "count": items.len(),
        "first": items
            .iter()
            .take(MAX_LIST_ITEMS_IN_SUMMARY)
            .map(|item| truncate_text(item, MAX_SUMMARY_TEXT_BYTES))
            .collect::<Vec<_>>(),
    })
}

/// Укорачивает каждую строку внутри значения (рекурсивно, ключи не трогает).
fn bound_strings(value: &mut Value, max_bytes: usize) {
    match value {
        Value::String(text) if text.len() > max_bytes => *text = truncate_text(text, max_bytes),
        Value::Array(items) => items.iter_mut().for_each(|item| bound_strings(item, max_bytes)),
        Value::Object(fields) => fields.values_mut().for_each(|field| bound_strings(field, max_bytes)),
        _ => {}
    }
}

/// Худшие случаи для тестов бюджета строки результата сценариев видоискателя.
#[cfg(test)]
#[path = "result_budget_worst_case.rs"]
pub mod worst_case;

#[cfg(test)]
#[path = "result_budget_tests.rs"]
mod tests;
