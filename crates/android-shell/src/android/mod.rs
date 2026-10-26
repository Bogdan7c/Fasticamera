//! Android-часть оболочки: JNI, NDK и потоки платформы.
//!
//! Весь `unsafe` crate-а — в safety-модулях с `// SAFETY:`: [`native_window`],
//! `motion_event`, `sensor_thread`, `thermal_ffi`. Остальные модули безопасны.

mod callback_policy;
mod haptic_query;
mod motion_event;
mod native_window;
mod natives;
mod platform_calls;
mod runtime;
mod sensor_thread;
mod services;
mod thermal_ffi;
mod thermal_monitor;

pub use natives::register_natives;
pub use runtime::{ShellHandles, ShellInstallError, install};
pub use services::AndroidShellServices;
pub use thermal_monitor::ThermalListenerState;
