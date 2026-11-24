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
//!
//! Публичный compute-набор (сессия 07, решение владельца): [`compute`], [`storage_image`],
//! [`host_buffer`], [`device_buffer`], [`gpu_timing`], [`compositor`], [`backdrop`], [`frame_sink`] —
//! чтобы `isp-vulkan` строил GPU-ISP на безопасных RAII-обёртках, не получая сырой `ash::Device`.
//!
//! Растровый набор (сессия 11, решение 52): [`raster`] (графический конвейер, проход рендеринга,
//! смешивание), [`color_target`] (промежуточная цветовая цель: рисуем → читаем сэмплером),
//! [`sampled_texture`] (неизменяемая текстура, например SDF-атлас) — чтобы `ui-render-vulkan` рисовал
//! UI и стекло по тем же правилам, без сырого `ash::Device`.

pub mod backdrop;
mod checksum;
pub mod color_target;
pub mod compositor;
pub mod compute;
pub mod context;
pub mod device;
pub mod device_buffer;
pub mod encoder_input;
pub mod error;
pub mod frame_ring;
pub mod frame_sink;
pub mod gpu_timing;
pub mod hardware_buffer_cache;
pub mod host_buffer;
pub mod host_readback;
pub mod instance;
mod linear_sampler;
pub mod live_objects;
pub mod offscreen;
pub mod one_shot_queue;
pub mod pipeline_cache;
pub mod raster;
pub mod raw_mosaic_preview;
pub mod rgba8_image;
pub mod sampled_texture;
pub mod saturation;
pub mod shaders;
pub mod source_window;
pub mod storage_image;
pub mod swapchain;
pub mod sync;
pub mod test_pattern;
pub mod validation;

/// Блокирующие помощники: разовая отправка с ожиданием, чтение storage-картинки и host-буфера. Нужны тестам других
/// crate (сессия 07) и дайджесту стенда насыщения (16a, `saturation::digest`: он обязан работать в release без
/// фич). На горячем пути ожиданий GPU здесь нет: вызывать только вне кадровых циклов.
pub mod kit_test_support;

/// Только Android: импорт AHardwareBuffer и функции NDK для него (решение 32).
#[cfg(target_os = "android")]
pub mod android;
