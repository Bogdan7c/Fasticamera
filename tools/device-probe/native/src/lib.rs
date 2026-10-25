//! `probe-native`: нативная часть device-probe (Fasticamera).
//!
//! Цель — выяснить на реальном телефоне, можно ли RAW-буферы камеры (`AImageReader`) без
//! копирования импортировать в Vulkan (`VK_ANDROID_external_memory_android_hardware_buffer`),
//! читать compute-шейдером и корректно синхронизироваться через sync_fd.
//!
//! Раскладка:
//! * чистая логика (собирается и тестируется на host): `checksum`, `camera_metadata`,
//!   `status_names`, `hardware_buffer_names`, `gpu_timing`, `report`, `error`, `jni_boundary`;
//! * платформенный код (`target_os = "android"`): `camera_ndk`, `image_reader`, `vulkan`,
//!   `jni_exports`, `logging`.

pub mod camera_metadata;
pub mod checksum;
pub mod codec_surface;
pub mod error;
pub mod gpu_timing;
pub mod hardware_buffer_names;
pub mod jni_boundary;
pub mod report;
pub mod status_names;

#[cfg(target_os = "android")]
pub mod camera_ndk;
#[cfg(target_os = "android")]
pub mod image_reader;
#[cfg(target_os = "android")]
mod jni_exports;
#[cfg(target_os = "android")]
mod logging;
#[cfg(target_os = "android")]
pub mod vulkan;
