//! Android-часть оболочки: JNI, NDK и потоки платформы.
//!
//! Весь `unsafe` crate-а — в safety-модулях с `// SAFETY:`: [`native_window`],
//! `motion_event`, `sensor_thread`, `choreographer_thread`, `thermal_ffi`, `performance_hint_ffi`. Остальные модули безопасны
//! (`power_sampler` — поток опроса батареи через JNI, без `unsafe`).

mod callback_policy;
mod choreographer_thread;
mod haptic_query;
mod motion_event;
mod native_window;
mod natives;
mod performance_hint_ffi;
mod platform_calls;
mod power_sampler;
mod runtime;
mod sensor_thread;
mod services;
mod thermal_ffi;
mod thermal_monitor;

pub use natives::register_natives;
pub use performance_hint_ffi::AndroidPerformanceHints;
pub use runtime::{ShellHandles, ShellInstallError, install};
pub use services::AndroidShellServices;
pub use thermal_monitor::ThermalListenerState;
