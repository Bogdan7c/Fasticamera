//! `fasticamera-app` — composition root Fasticamera: нативная библиотека `libfasticamera.so`.
//!
//! Сейчас (сессия 01) это каркас: точка входа JNI, логирование в logcat, перехват паник
//! и smoke-сценарии для `scripts/device-smoke.sh`. Логики фич здесь нет и не будет:
//! crate только собирает модули и маршрутизирует вызовы (README плана, «Владельцы состояния»).
//!
//! Раскладка:
//! * платформенно-нейтральная часть (собирается и тестируется на ПК): `runtime_config`,
//!   `build_info`, `logging` (кроме приёмника logcat), `panic_report`, `jni_boundary`,
//!   `scenario`, `result_line`, `runtime`, `event_router` (маршрутизатор событий оболочки),
//!   `render_route`/`render_wiring` (передача surface потоку рендера), `gpu_services`, `gpu_config`,
//!   `camera_config`/`camera_wiring`/`camera_service`/`camera_lifecycle` (движок камеры, его политика
//!   жизненного цикла и сервис для сценариев);
//! * только Android: `jni_exports` (`JNI_OnLoad`, `RegisterNatives`), `shell_wiring`
//!   (установка `android-shell`, запуск потоков рендера и камеры и поток маршрутизатора), `vulkan_diagnostics`
//!   (разовые GPU-проверки на `gpu-vulkan`) и приёмник `logging::logcat_sink`.

pub mod build_info;
pub mod camera_config;
pub mod camera_lifecycle;
pub mod camera_presentation;
pub mod camera_scenario_config;
pub mod camera_service;
#[cfg(test)]
mod camera_test_support;
pub mod camera_wiring;
pub mod clock_offset;
pub mod event_router;
pub mod gpu_config;
pub mod gpu_services;
#[cfg(test)]
mod gpu_test_support;
pub mod jni_boundary;
pub mod logging;
pub mod panic_report;
pub mod power_measurement;
pub mod raw_stream_scenario_config;
pub mod raw_stream_services;
#[cfg(test)]
mod raw_stream_test_support;
pub mod raw_stream_wiring;
pub mod render_route;
pub mod render_wiring;
pub mod result_echo;
pub mod result_line;
pub mod runtime;
pub mod runtime_config;
pub mod scenario;
pub mod viewfinder_autostart;
pub mod viewfinder_control;
pub mod viewfinder_latency_profile;
pub mod viewfinder_pacing_control;
pub mod viewfinder_scenario_config;

#[cfg(target_os = "android")]
mod jni_exports;
#[cfg(target_os = "android")]
mod shell_wiring;
#[cfg(target_os = "android")]
mod vulkan_diagnostics;
