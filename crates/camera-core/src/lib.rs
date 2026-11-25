//! `camera-core` — движок камеры Fasticamera без Android.
//!
//! Один владелец камеры с явной машиной состояний (сессия 04): UI и рендер не обрабатывают
//! «камера отвалилась» каждый по-своему, а видят [`CameraState`] и [`CameraEvent`].
//!
//! Устройство:
//! * команды меняют **желаемое** состояние (какая камера, режим, идёт ли поток, параметры);
//!   движок сам доводит до него реальное — и снова доводит после вытеснения или сбоя HAL
//!   (решение 39);
//! * с платформой движок говорит только через трейт [`backend::CameraBackend`]; реализация
//!   на Camera2 NDK — crate `camera-android`, для тестов — fake;
//! * бэкенд живёт на потоке `fasticamera-camera` и только там; его асинхронные события
//!   (колбэки NDK с потоков binder) попадают в почтовый ящик этого потока и никогда
//!   не блокируют отправителя;
//! * снаружи никто не ждёт камеру синхронно (инвариант 4 README): команды уходят в ограниченную
//!   очередь, итоги и смены состояния приходят событиями, а снимок [`CameraStatus`] всегда
//!   актуален.
//!
//! Инвариант: crate собирается и тестируется на Linux без телефона (инвариант 5 README,
//! guardrail `neutral-no-platform`).

pub mod backend;
pub mod backend_standby;
pub mod cadence;
pub mod command;
pub mod config;
pub mod engine;
pub mod event;
pub mod exposure_transfer;
#[cfg(any(test, feature = "fake-backend"))]
pub mod fake;
pub mod flash_config;
pub mod flash_status;
pub mod handle;
pub mod inbox;
pub mod lens_status;
pub mod lens_wiring;
pub mod request_controls;
pub mod role_classifier;
pub mod runtime;
pub mod standby_config;
pub mod standby_status;
pub mod state;
pub mod status;
pub mod three_a_control;
mod three_a_exposure_limits;
pub mod three_a_own;
pub mod three_a_status;
pub mod three_a_vendor;
pub mod three_a_wiring;
pub mod white;
mod white_take;
pub mod zoom;

pub use command::{CameraCommand, CommandId, CommandOutcome, CommandRejected};
pub use config::CameraEngineConfig;
pub use engine::CameraEngine;
pub use event::CameraEvent;
pub use handle::CameraHandle;
pub use inbox::{LensIntentWaker, ThreeAWaker};
pub use lens_status::{LensStatus, LensSwitchState};
pub use lens_wiring::LensWiring;
pub use runtime::{CameraRuntime, RuntimeSpawnError, ShutdownReport};
pub use state::{CameraError, CameraState, ClosedReason, RecoveryPlan};
pub use status::{AppPresence, CameraCounters, CameraStatus, StreamStats};
pub use three_a_wiring::ThreeAWiring;
pub use white::{WhiteConfig, WhiteStatus, WhiteWiring};
