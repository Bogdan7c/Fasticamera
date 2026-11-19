//! Код запекания шрифта и иконок. Подключается в `build.rs` и в тесты через `#[path]`, поэтому
//! модули зависят друг от друга только через `super::` и не знают, кто их подключил.
//!
//! * [`sdf`] — генератор SDF (заливка по обмотке, штрих); * [`pack`] — упаковка полками;
//! * [`manifest`] — разбор `assets/bake.json`; * [`font_bake`] — метрики, кернинг, контуры (harfrust, skrifa);
//! * [`glyph_cell`] — ячейка глифа; * [`svg`] — подмножество SVG и ячейка иконки (kurbo);
//! * [`codegen`] — исходный код таблиц; * [`bake`] — всё вместе.

// Каждый потребитель (build.rs, тесты) использует свою часть: «неиспользуемое» здесь — норма.
#![allow(dead_code)]

pub mod bake;
pub mod codegen;
pub mod error;
pub mod font_bake;
pub mod glyph_cell;
pub mod manifest;
pub mod pack;
pub mod sdf;
pub mod svg;
