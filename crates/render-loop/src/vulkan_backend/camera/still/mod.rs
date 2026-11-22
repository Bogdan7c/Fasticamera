//! Снимок без задержки затвора на стороне бэкенда (сессия 14, пакет P4b): выбор кадра «что было на экране»,
//! история параметров, очередь снимков и проявление по полосам между кадрами видоискателя.
//!
//! # Архитектура
//! ```text
//! видоискатель (CameraSession) ──StillFeed──▶ StillEngine ◀── PastPresentation (записи показа драйвера)
//!   показы (present_id → кадр), параметры         │  очередь, протокол событий, сроки
//!   обработанных кадров (белая, k)                ├─▶ кольцо снимков (pick: неразрушающий выбор кадра)
//!                                                 └─▶ StillGpu (копия RAW, проявление полосами, показ в кодер)
//! ```
//! * [`feed`] — лента от сессии видоискателя: единственное место, где снимок узнаёт, что стояло на экране и с чем
//!   обработан кадр (снимок не лезет в поля сессии);
//! * [`present_log`] — журнал показов: ДОКАЗАТЕЛЬСТВО «какой кадр был на экране в момент нажатия» по двум соседним
//!   записям драйвера; нет доказательства — оценка с явной пометкой;
//! * [`params_history`] — белая и `k` показанных кадров: снимок повторяет параметры СВОЕГО кадра;
//! * [`choice`], [`facts`], [`rotation`] — перевод выбора кольца в словарь снимка, факты кадра (кадрирование из
//!   плана, нейтраль DNG, поворот);
//! * [`gpu`] — узкая граница GPU; [`vulkan_gpu`] — настоящая реализация, поддельная — в тестах;
//! * [`entry`], [`engine`], [`engine_steps`] — очередь, итоги («ровно один на снимок»), шаги на кадр;
//! * `engine_surface` — жизнь без surface: что бросается при отсоединении, что доводится, когда отпускается GPU;
//! * [`diagnostics`] — счётчики.
//!
//! # Инварианты
//! 1. Каждый принятый `Begin` кончается РОВНО одним итоговым событием (см. `entry`).
//! 2. `ShownAtPress` строится только из доказанной записи драйвера, иначе `PresentationTimingUnknown`.
//! 3. Поток рендера не ждёт GPU снимков (опрос), кроме ограниченного ожидания при остановке.
//! 4. Пропажа surface не бросает захваченные снимки (решения 146, 150): они доводятся без экрана по таймеру потока
//!    рендера, бросаются только не захваченные (`engine_surface`). Когда без surface очередь опустела, объектов GPU
//!    снимков не остаётся; очередь и счётчики живут на уровне бэкенда.
//! 5. Часы не смешиваются: нажатие и показ — `MONOTONIC`, метки сенсора — `BOOTTIME`.

pub mod choice;
pub mod diagnostics;
pub mod engine;
mod engine_present;
mod engine_steps;
mod engine_surface;
pub mod entry;
pub mod facts;
pub mod feed;
pub mod gpu;
mod gpu_loss;
pub mod params_history;
pub mod present_log;
pub mod rotation;
mod step_timing;
pub mod vulkan_gpu;

#[cfg(test)]
mod engine_loss_tests;
#[cfg(test)]
mod engine_queue_tests;
#[cfg(test)]
mod engine_surface_tests;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod fake_gpu;
#[cfg(test)]
mod test_support;

pub use self::diagnostics::StillDiagnostics;
pub use self::engine::{GpuMaker, StillEngine, SystemStillClock};
pub use self::feed::{FrameStillParams, ShownNow, StillFeed, StillFeedRecorder};
pub use self::vulkan_gpu::{VulkanStillGpu, VulkanStillSettings};

/// Цель журнала `tracing` для снимков.
pub(crate) const LOG_TARGET: &str = "fasticamera::render::still";
