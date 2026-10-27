//! `gpu-vulkan` — Vulkan-ядро Fasticamera на `ash` (решение 7, сессия 03).
//!
//! Всё «под капотом» стоит на этом crate: видоискатель, ISP, UI, стекло. Ошибки владения
//! ресурсами в Vulkan проявляются не сразу и в чужих местах, поэтому владение и синхронизация
//! спроектированы здесь один раз: каждый объект Vulkan — RAII-тип со счётчиком живых объектов,
//! освобождение — в правильном порядке через владение, а не через ручные `destroy_*` у вызывающих.
//!
//! Инварианты (README плана `foundation`, «Владельцы состояния»):
//! * crate не знает про камеру и алгоритмы ISP (guardrail `gpu-not-camera`);
//! * `jni`/`ndk-sys` не используются; нужные функции NDK для AHardwareBuffer объявлены в одном
//!   safety-модуле `android::hardware_buffer_ffi` (решение 32);
//! * `unsafe` — только в safety-модулях с `// SAFETY:`.

pub mod context;
pub mod device;
pub mod error;
pub mod frame_ring;
pub mod hardware_buffer_cache;
pub mod instance;
pub mod live_objects;
pub mod offscreen;
pub mod raw_mosaic_preview;
pub mod shaders;
pub mod swapchain;
pub mod sync;
pub mod test_pattern;
pub mod validation;

/// Только Android: импорт AHardwareBuffer и функции NDK для него (решение 32).
#[cfg(target_os = "android")]
pub mod android;
