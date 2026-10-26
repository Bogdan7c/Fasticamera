//! Только Android: импорт `AHardwareBuffer` в Vulkan и диагностика этого пути (решение 32).
//!
//! * [`hardware_buffer_ffi`] — единственное место `extern "C"` в crate (guardrail
//!   `gpu-ffi-only-in-safety-module`): сырые объявления функций NDK;
//! * [`hardware_buffer`] — безопасная обёртка со счётом ссылок ([`hardware_buffer::HardwareBufferRef`]);
//! * [`import`] — импорт буфера как `VkImage R16_UINT` или `VkBuffer`
//!   ([`import::ImportedHardwareBuffer`]);
//! * [`import_cache`] — кэш импортов по `AHardwareBuffer_getId` поверх нейтральной логики
//!   [`crate::hardware_buffer_cache`];
//! * [`diagnostics`] — `ahb_roundtrip` для smoke-сценария `ahb-import`.
//!
//! Нейтральные части (раскладки, проверки размеров, кэш, запуск контрольных сумм, барьеры передачи
//! владения) живут в [`crate::hardware_buffer_cache`] и тестируются на ПК.

pub mod diagnostics;
pub mod hardware_buffer;
pub mod hardware_buffer_ffi;
pub mod import;
pub mod import_cache;
