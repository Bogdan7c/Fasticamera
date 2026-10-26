//! `camera-ndk-sys` — сырые биндинги Camera2 NDK (`libcamera2ndk`) и `AImageReader` (`libmediandk`).
//!
//! Биндинги генерирует `bindgen` в `build.rs` по заголовкам закреплённого NDK (решение 37,
//! версия — `fasticamera.ndkVersion` в `android/gradle.properties`); готовый `ndk-sys` камере
//! запрещён guardrail-ом `jni-ndk-only-in-shell`. Что попадает в биндинги, определяет список
//! разрешённых имён в `build.rs` (камера, метаданные, запрос, сессия, `AImageReader`/`AImage`,
//! коды ошибок, теги `ACAMERA_*`).
//!
//! Перечисления C генерируются как **константы**, а не как Rust-`enum`: HAL и фреймворк могут
//! вернуть значение, которого нет в заголовках NDK, и Rust-`enum` с таким значением — UB.
//!
//! На хосте (не Android) crate пустой: нейтральная логика `camera-android` собирается и
//! тестируется на ПК без NDK. Единственное, что собирается на хосте, — тесты поиска NDK.

#[cfg(target_os = "android")]
mod bindings {
    // Генерат bindgen: имена и стиль C, неиспользуемые элементы и предупреждения clippy в нём
    // нам неподконтрольны. Здесь именно `allow`, а не `expect`: `expect` падал бы с
    // «unfulfilled lint expectation», стоит заголовкам NDK смениться так, что какое-то
    // предупреждение исчезнет. Весь остальной код crate под обычными линтами.
    #![allow(
        non_camel_case_types,
        non_upper_case_globals,
        non_snake_case,
        dead_code,
        unused_imports,
        clippy::all,
        clippy::pedantic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        unnecessary_transmutes,
        unsafe_op_in_unsafe_fn
    )]

    include!(concat!(env!("OUT_DIR"), "/ndk_bindings.rs"));
}

#[cfg(target_os = "android")]
pub use bindings::*;

// Чистая логика поиска NDK и libclang — общий файл с build.rs; тесты идут в обычном `cargo test`.
#[cfg(test)]
#[path = "../build_support/ndk_toolchain.rs"]
mod ndk_toolchain;
