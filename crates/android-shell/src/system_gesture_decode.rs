//! Разбор `Insets` обязательных системных жестов Android в нейтральное [`SystemGestureZones`].
//!
//! Kotlin берёт `WindowInsets.getInsetsIgnoringVisibility(Type.mandatorySystemGestures())` и пересылает
//! четыре числа **в порядке `android.graphics.Insets`**: `left, top, right, bottom`. Здесь порядок
//! закреплён именованными полями ([`RawEdgeInsetsPx`]) и тестами на ПК: перепутанные края дали бы зону
//! не там, где она есть, — и тихо «съели» бы чужие касания.

use shell_contract::system_gesture::{EdgeInsetsError, RawEdgeInsetsPx, SystemGestureZones};

/// Разбирает толщины полос от платформы. Отрицательная толщина — ошибка платформы или обвязки
/// (вызывающий пишет лог и счётчик `native_callback_failures`), а не «обрежем до нуля».
pub fn decode_system_gesture_zones(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) -> Result<SystemGestureZones, EdgeInsetsError> {
    SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left,
        top,
        right,
        bottom,
    })
}

#[cfg(test)]
#[path = "system_gesture_decode_tests.rs"]
mod tests;
