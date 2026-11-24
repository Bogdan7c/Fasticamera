//! Разбор `Insets` безопасной зоны Android в нейтральное [`SafeAreaInsets`].
//!
//! Kotlin берёт `WindowInsets.getInsetsIgnoringVisibility(systemBars | displayCutout | mandatorySystemGestures)` —
//! платформа сама объединяет виды отступов, по наибольшей толщине у каждого края, — и пересылает четыре числа
//! **в порядке `android.graphics.Insets`**: `left, top, right, bottom`. Порядок закреплён именованными полями и
//! тестами на ПК: перепутанные края увели бы раскладку от чужого края и оставили управление под вырезом.

use shell_contract::edge_insets::{EdgeInsetsError, RawEdgeInsetsPx};
use shell_contract::safe_area::SafeAreaInsets;

/// Разбирает отступы платформы. Отрицательный отступ — ошибка платформы или обвязки (вызывающий пишет лог и
/// счётчик `native_callback_failures`, прежняя зона остаётся), а не «обрежем до нуля».
pub fn decode_safe_area(left: i32, top: i32, right: i32, bottom: i32) -> Result<SafeAreaInsets, EdgeInsetsError> {
    SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left,
        top,
        right,
        bottom,
    })
}

#[cfg(test)]
#[path = "safe_area_decode_tests.rs"]
mod tests;
