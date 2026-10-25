//! Работа с `AImageReader`: реестр ридеров по opaque-handle, обёртка над acquire/release
//! `AImage`, CPU-lock `AHardwareBuffer` и сам прогон «импорт в Vulkan».
//! Весь unsafe — в `registry`, `acquired_image`, `ahb_lock`, `surface`.

mod acquired_image;
mod ahb_lock;
mod frame_pipeline;
mod import_test;
mod registry;
mod surface;

pub use import_test::run_import_test_json;
pub use registry::{ReaderParams, create_reader_json, destroy_reader_json};
pub use surface::reader_surface;
