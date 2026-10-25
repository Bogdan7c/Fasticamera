//! Тест «Vulkan рендерит прямо во входную Surface `MediaCodec`» (без копий через CPU).
//!
//! Раскладка: чистая логика (`policy`, `report`) собирается и на host; платформенные части
//! (`window`, `instance`, `surface_info`, `swapchain`, `render_loop`) — только Android.
//! Вход — `Surface` от `MediaCodec.createInputSurface()`; выход — JSON-отчёт по шагам.

pub mod policy;
pub mod report;

#[cfg(target_os = "android")]
mod instance;
#[cfg(target_os = "android")]
mod render_loop;
#[cfg(target_os = "android")]
mod session;
#[cfg(target_os = "android")]
mod surface_info;
#[cfg(target_os = "android")]
mod swapchain;
#[cfg(target_os = "android")]
pub mod window;

#[cfg(target_os = "android")]
pub use session::run_codec_surface_test_json;
