//! Подробности отчётов сценариев камеры: строки стадий вместо одной раздутой строки результата.
//!
//! Строка результата (`FASTICAMERA_RESULT`) — одна запись logcat, не длиннее
//! [`LOGCAT_MAX_TEXT_BYTES`](crate::logging::record::LOGCAT_MAX_TEXT_BYTES) (4000 байт), иначе она
//! заменяется отчётом об ошибке. Полный каталог четырёх камер или шесть шагов замера в неё не влезают.
//! Поэтому в результате лежит сжатая сводка (всё, что нужно проверкам и вердикту), а полные данные
//! уходят строками стадий `detail_*` перед результатом: элементы укладываются в строки не длиннее бюджета,
//! скрипт `smoke_wait.py` собирает эти строки и печатает после вердикта.
//!
//! Формат строки: стадия `detail_<имя>` с полями `part` (с 1), `parts` и `items` (массив элементов).

use serde_json::Value;

use crate::scenario::observing::{ScenarioContext, stage_fields};

/// Сколько байт элементов кладём в одну строку стадии. Запись logcat — 4000 байт; из них префикс
/// `FASTICAMERA_STAGE `, имя сценария и стадии, `part`/`parts` и скобки — до ~200 байт. 3400 — с запасом.
pub const DETAIL_LINE_BUDGET_BYTES: usize = 3_400;

/// Сколько записей журнала событий движка попадает в отчёт: журнал полного прогона может быть длинным.
pub const EVENTS_IN_REPORT_LIMIT: usize = 30;

/// Раскладывает элементы по строкам жадно, в порядке следования. Элемент, который один не влезает в
/// бюджет, заменяется записью о размере: потерять элемент молча нельзя, а резать JSON пополам — тоже.
pub fn pack_items(items: Vec<Value>, budget_bytes: usize) -> Vec<Vec<Value>> {
    let mut lines: Vec<Vec<Value>> = Vec::new();
    let mut current: Vec<Value> = Vec::new();
    let mut current_bytes = 0_usize;
    for item in items {
        let item = fit_single(item, budget_bytes);
        // +1 — запятая между элементами массива.
        let item_bytes = item.to_string().len() + 1;
        if !current.is_empty() && current_bytes + item_bytes > budget_bytes {
            lines.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes += item_bytes;
        current.push(item);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn fit_single(item: Value, budget_bytes: usize) -> Value {
    let bytes = item.to_string().len();
    // +1 — запятая между элементами массива.
    if bytes < budget_bytes {
        return item;
    }
    serde_json::json!({ "oversized_item_bytes": bytes, "budget_bytes": budget_bytes })
}

/// Пишет элементы строками стадии `stage` (по возможности несколькими).
pub fn emit_details(context: &mut ScenarioContext<'_>, stage: &'static str, items: Vec<Value>) {
    let lines = pack_items(items, DETAIL_LINE_BUDGET_BYTES);
    let parts = lines.len();
    for (index, line) in lines.into_iter().enumerate() {
        context.emit_stage(
            stage,
            stage_fields([
                ("part", (index + 1).into()),
                ("parts", parts.into()),
                ("items", Value::Array(line)),
            ]),
        );
    }
}

/// Значение для отчёта; сбой сериализации не теряется, а становится записью-объяснением.
pub fn to_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_else(|error| serde_json::json!({ "serialization_error": error.to_string() }))
}

#[cfg(test)]
#[path = "detail_tests.rs"]
mod tests;
