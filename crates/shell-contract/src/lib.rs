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
//! * [`hardware_key`] — аппаратные клавиши (громкость как затвор);
//! * [`system_gesture`] — зоны обязательных системных жестов по краям экрана;
//! * [`touch`], [`orientation`], [`permission`] — касания, физическая ориентация, разрешение камеры;
//! * [`thermal`] и [`thermal_policy`] — показания тепла и единственный владелец тепловых решений;
//! * [`haptics`], [`commands`] — намерения к платформе (вибрация, разрешение, частота кадров…);
//! * [`operator_prompt`] — подсказка владельцу на экране (сигнал «сделай руками» вместо вибрации);
//! * [`motion`] — гироскоп (кольцо последних секунд) и акселерометр для ориентации;
//! * [`vsync`] — тики vsync и линии кадра платформы (пейсинг рендера);
//! * [`diagnostics`] — счётчики шва для smoke-проверок;
//! * [`sync_fence`] — владение sync_fd: fence ожидания перед GPU и fence освобождения после GPU;
//! * [`hardware_buffer`] — заимствованный `AHardwareBuffer` кадра камеры для импорта в GPU без копии;
//! * [`power`] — показания батареи (ток, заряд, температура) для замера цены стадий ISP.

pub mod clock;
pub mod commands;
pub mod diagnostics;
pub mod event;
pub mod event_queue;
pub mod gravity;
pub mod haptics;
pub mod hardware_buffer;
pub mod hardware_key;
pub mod motion;
pub mod operator_prompt;
pub mod orientation;
pub mod performance_hint;
pub mod permission;
pub mod power;
pub mod surface;
pub mod sync_fence;
pub mod system_gesture;
pub mod thermal;
pub mod thermal_policy;
pub mod touch;
pub mod vsync;
