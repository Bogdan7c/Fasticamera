//! Синхронизация CPU/GPU и между подсистемами: `sync_fd`, timeline-семафоры, сборка сабмитов.
//!
//! Модуль владеет всей логикой «между работой на GPU»:
//! * [`import_wait_fence`] — acquire-fence камеры ([`GpuWaitFence`]) → бинарный семафор для
//!   ожидания в сабмите; при отказе fence возвращается вызывающему (CPU-ожидание);
//! * [`ExportableSemaphore`] + [`PendingReleaseSignal`] — release-fence после работы GPU
//!   ([`GpuReleaseFence`]); экспорт возможен только после сабмита (проверяется типами);
//! * [`TimelineSemaphore`] — внутренние зависимости между сабмитами с монотонным счётчиком;
//! * [`QueueSubmission`] — сборщик `VkSubmitInfo2` с именованными намерениями;
//! * [`wait_release_fence_on_cpu`] — ожидание release-fence на CPU без `libc`;
//! * [`diagnostics`] — проверка порядка «работа A → sync_fd → работа B» для ПК и телефона.
//!
//! Типы `GpuWaitFence`/`GpuReleaseFence` живут в нейтральном `shell-contract::sync_fence`,
//! чтобы слой камеры передавал их GPU, не завися от этого crate.

mod cpu_wait;
pub mod diagnostics;
mod import;
mod release;
mod submit;
mod timeline;

pub use self::cpu_wait::{FenceWait, wait_release_fence_on_cpu};
pub use self::import::{FenceImportError, WaitSemaphore, import_wait_fence};
pub use self::release::{ExportableSemaphore, PendingReleaseSignal};
pub use self::submit::{QueueSubmission, SubmittedWork};
pub use self::timeline::{TimelineSemaphore, TimelineWait};
pub use shell_contract::sync_fence::{GpuReleaseFence, GpuWaitFence, SyncFenceError};
