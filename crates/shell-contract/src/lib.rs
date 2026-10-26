//! `shell-contract` — нейтральный словарь шва между Android и Rust (решение владельца, сессия 02).
//!
//! Шов Android ↔ Rust — главная граница приложения. Всё, что выше шва (`ui-core`,
//! `camera-screen`, `camera-core`, рендер), видит только типы и трейты отсюда, а не JNI и NDK.
//! Реализация для телефона — crate `android-shell`, для тестов — fake-реализации трейтов.
//!
//! Инвариант: crate не зависит ни от Android, ни от Vulkan, ни от других crate-ов workspace
//! (`scripts/crate-boundaries.toml`, правила `neutral-no-platform` и `contract-is-leaf`).
//!
//! Раскладка:
//! * [`clock`] — две шкалы времени Android как разные типы;
//! * [`event`] и [`event_queue`] — события оболочки и ограниченная очередь с учётом потерь;
//! * [`surface`] — surface экрана и квитанция её освобождения;
//! * [`touch`], [`orientation`], [`permission`] — касания, физическая ориентация, разрешение камеры;
//! * [`thermal`] и [`thermal_policy`] — показания тепла и единственный владелец тепловых решений;
//! * [`haptics`], [`commands`] — намерения к платформе (вибрация, разрешение, частота кадров…);
//! * [`motion`] — гироскоп (кольцо последних секунд) и акселерометр для ориентации;
//! * [`diagnostics`] — счётчики шва для smoke-проверок.

pub mod clock;
pub mod commands;
pub mod diagnostics;
pub mod event;
pub mod event_queue;
pub mod haptics;
pub mod motion;
pub mod orientation;
pub mod permission;
pub mod surface;
pub mod thermal;
pub mod thermal_policy;
pub mod touch;
