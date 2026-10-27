//! Лизинг кадра: расписка потребителя за буфер камеры.
//!
//! Один кадр могут одновременно держать несколько потребителей (видоискатель и кольцо снимков):
//! у каждого свой [`RawFrameLease`], а сам буфер — один ([`SharedFrame`]). Буфер уходит
//! производителю, когда последний держатель вернул свой лизинг; fence-ы всех держателей
//! передаются вместе.

use std::ffi::c_void;
use std::io;
use std::mem;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_contract::frame_passport::RawFramePassport;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::sync_fence::{GpuReleaseFence, GpuWaitFence};

use crate::counters::StreamAccounting;
use crate::producer_frame::{ProducerFrame, ReleaseFences};

/// Буфер кадра, общий для всех держателей.
pub(crate) struct SharedFrame {
    passport: RawFramePassport,
    /// `AHardwareBuffer*`, снятый с [`ProducerFrame::hardware_buffer`] при создании. Живой, пока
    /// производитель не получил кадр назад (`state.producer` ещё `Some`) — контракт трейта.
    buffer: NonNull<c_void>,
    acquire_fence: GpuWaitFence,
    state: Mutex<ShareState>,
    accounting: Arc<StreamAccounting>,
}

// SAFETY: `buffer` — указатель на `AHardwareBuffer` со счётчиком ссылок и потокобезопасными
// функциями NDK; остальное — `Send + Sync` типы или данные под `Mutex`. Производитель внутри
// (`Box<dyn ProducerFrame>`, он `Send`) доступен только под замком.
unsafe impl Send for SharedFrame {}
// SAFETY: см. выше; через `&SharedFrame` указатель только читается.
unsafe impl Sync for SharedFrame {}

struct ShareState {
    /// Сколько лизингов на этот буфер ещё не возвращено.
    holders: usize,
    release: ReleaseFences,
    /// Хоть один держатель выбросил лизинг без возврата: состояние GPU неизвестно.
    leaked: bool,
    /// `None` — буфер уже отдан производителю или утёк.
    producer: Option<Box<dyn ProducerFrame>>,
}

impl SharedFrame {
    /// Принимает кадр в лизинг и выдаёт первую расписку.
    pub(crate) fn lease(
        passport: RawFramePassport,
        producer: Box<dyn ProducerFrame>,
        acquire_fence: GpuWaitFence,
        accounting: Arc<StreamAccounting>,
    ) -> RawFrameLease {
        let buffer = producer.hardware_buffer().as_ptr();
        accounting.note_frame_leased();
        let frame = Arc::new(Self {
            passport,
            buffer,
            acquire_fence,
            state: Mutex::new(ShareState {
                holders: 1,
                release: ReleaseFences::default(),
                leaked: false,
                producer: Some(producer),
            }),
            accounting,
        });
        RawFrameLease { frame, settled: false }
    }

    fn lock_state(&self) -> MutexGuard<'_, ShareState> {
        // Замок держится только на счёт ссылок, паника под ним не оставляет полусостояния,
        // поэтому «отравленный» замок безопасно продолжать.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Как держатель вернул свой лизинг.
enum Settlement {
    AfterGpu(GpuReleaseFence),
    Unused,
    /// Лизинг выброшен без возврата (решение 43).
    Dropped,
}

/// Расписка потребителя за кадр камеры. Вернуть — только явно:
/// [`release_after_gpu_fence`](Self::release_after_gpu_fence) или [`release_unused`](Self::release_unused).
/// Выброшенный без возврата лизинг — утечка буфера со счётчиком `leaked_leases` (решение 43).
#[must_use = "лизинг надо вернуть явно: release_after_gpu_fence или release_unused"]
pub struct RawFrameLease {
    frame: Arc<SharedFrame>,
    /// Лизинг уже возвращён явно — `Drop` ничего не делает.
    settled: bool,
}

impl RawFrameLease {
    /// Ещё одна расписка на тот же буфер (для следующего потребителя).
    pub(crate) fn share(&self) -> RawFrameLease {
        self.frame.lock_state().holders += 1;
        RawFrameLease {
            frame: Arc::clone(&self.frame),
            settled: false,
        }
    }

    pub fn passport(&self) -> &RawFramePassport {
        &self.frame.passport
    }

    /// Буфер для импорта в GPU. Действителен, пока жив этот лизинг; GPU, которому буфер нужен
    /// дольше, берёт свою ссылку NDK (импорт в Vulkan так и делает).
    pub fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        // SAFETY: пока жив хоть один лизинг, `holders > 0`, значит производитель кадр назад
        // не получил и по контракту `ProducerFrame` указатель живой; заимствование привязано
        // ко времени жизни `&self`.
        unsafe { BorrowedHardwareBuffer::from_raw(self.frame.buffer) }
    }

    /// Acquire-fence кадра (своя копия): GPU обязан дождаться его перед чтением.
    /// На OnePlus 11 HAL отдаёт буфер уже готовым (00 C4), копия тогда без системного вызова.
    pub fn acquire_fence(&self) -> io::Result<GpuWaitFence> {
        self.frame.acquire_fence.try_clone()
    }

    /// Вернуть кадр после работы GPU: буфер уйдёт камере только по сигналу `fence`
    /// (и fence-ов остальных держателей).
    pub fn release_after_gpu_fence(mut self, fence: GpuReleaseFence) {
        self.settle(Settlement::AfterGpu(fence));
    }

    /// Вернуть кадр, к которому GPU не прикасался (вытеснен, не понадобился).
    pub fn release_unused(mut self) {
        self.settle(Settlement::Unused);
    }

    fn settle(&mut self, settlement: Settlement) {
        self.settled = true;
        let accounting = &self.frame.accounting;
        let mut state = self.frame.lock_state();
        match settlement {
            Settlement::AfterGpu(fence) => {
                accounting.note_released_after_fence();
                state.release.push(fence);
            }
            Settlement::Unused => accounting.note_released_unused(),
            Settlement::Dropped => {
                accounting.note_leaked_lease();
                state.leaked = true;
            }
        }
        state.holders = state.holders.saturating_sub(1);
        if state.holders > 0 {
            return;
        }
        let Some(producer) = state.producer.take() else {
            return;
        };
        let leaked = state.leaked;
        let release = mem::take(&mut state.release);
        // Производителя зовём вне замка: `AImage_deleteAsync` берёт замок reader-а.
        drop(state);
        if leaked {
            // GPU мог ещё читать буфер: вернуть его камере — значит дать HAL писать под GPU.
            // Лучше утечка, чем порча кадра (решение 43); видно по счётчику и в логе.
            tracing::error!(
                target: "fasticamera::raw_stream",
                sequence = self.frame.passport.sequence.0,
                "лизинг выброшен без возврата: буфер камеры утёк"
            );
            accounting.note_frame_leaked();
            Box::leak(producer);
        } else {
            producer.return_to_producer(release);
            accounting.note_frame_returned();
        }
    }
}

impl Drop for RawFrameLease {
    fn drop(&mut self) {
        if !self.settled {
            self.settle(Settlement::Dropped);
        }
    }
}

impl std::fmt::Debug for RawFrameLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawFrameLease")
            .field("sequence", &self.frame.passport.sequence)
            .field("sensor_timestamp", &self.frame.passport.sensor_timestamp)
            .finish()
    }
}
