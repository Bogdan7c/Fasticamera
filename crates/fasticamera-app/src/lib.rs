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

pub mod ae_trace_scenario_config;
pub mod app_directory;
pub mod auto_exposure_link;
pub mod build_info;
pub mod camera_config;
pub mod camera_lifecycle;
pub mod camera_presentation;
pub mod camera_scenario_config;
pub mod camera_screen_capture;
pub mod camera_screen_config;
pub mod camera_screen_effects;
pub mod camera_screen_facts;
pub mod camera_screen_gravity;
pub mod camera_screen_input;
pub mod camera_screen_lens_facts;
pub mod camera_screen_service;
pub mod camera_screen_source;
mod camera_screen_white_facts;
pub mod camera_screen_white_view;
pub mod camera_service;
#[cfg(test)]
mod camera_test_support;
pub mod camera_white_effects;
pub mod camera_wiring;
pub mod capture_backend;
pub mod capture_scenario_config;
mod capture_switch;
pub mod clock_offset;
pub mod device_config;
pub mod event_router;
#[cfg(test)]
mod fake_render_ae;
#[cfg(test)]
mod fake_render_white;
pub mod flash_camera_port;
pub mod flash_shot_route;
pub mod flash_still_relay;
#[cfg(test)]
mod flash_test_support;
pub mod glass_scenario_config;
pub mod gpu_config;
pub mod gpu_services;
#[cfg(test)]
mod gpu_test_support;
pub mod jni_boundary;
pub mod lens_shading_profile;
pub mod logging;
pub mod look_effects;
pub mod look_facts;
pub mod look_service;
pub mod look_startup;
#[cfg(test)]
mod look_test_support;
pub mod look_wheel_ring;
#[cfg(test)]
mod operator_prompt_fake;
#[cfg(test)]
mod orientation_sensor_fake;
pub mod panic_report;
pub mod photo_config;
pub mod pipeline_cache_file;
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
pub mod saturation_bench;
pub mod saturation_scenario_config;
pub mod scenario;
pub mod scope_effects;
pub mod screen_scenario_config;
pub mod stage_budget_surface_config;
pub mod still_camera_catalog;
pub mod still_capture_service;
pub mod still_choice_log;
pub mod still_event_forwarder;
pub mod still_phase_log;
pub mod still_render_port;
pub mod still_runtime;
pub mod still_screen_ledger;
#[cfg(test)]
mod still_test_support;
pub mod still_wall_clock;
pub mod text_effects;
pub mod text_input_broker;
pub mod three_a_scenario_config;
#[cfg(test)]
mod three_a_test_hal;
pub mod viewfinder_autostart;
pub mod viewfinder_control;
pub mod viewfinder_latency_profile;
pub mod viewfinder_pacing_control;
pub mod viewfinder_scenario_config;
pub mod white_memory;
pub mod white_sources;
pub mod zoom_scenario_config;

#[cfg(target_os = "android")]
mod jni_exports;
#[cfg(target_os = "android")]
mod shell_wiring;
#[cfg(target_os = "android")]
mod still_android_wiring;
#[cfg(target_os = "android")]
mod vulkan_diagnostics;
