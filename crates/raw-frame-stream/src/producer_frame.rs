//! Кадр производителя: то, что камера отдаёт в лизинг, и то, как он к ней возвращается.

use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::sync_fence::GpuReleaseFence;

/// Буфер кадра от производителя (на телефоне — `AImage` из `AImageReader`).
///
/// # Safety
/// Реализация гарантирует: указатель из [`hardware_buffer`](Self::hardware_buffer) остаётся живым
/// `AHardwareBuffer*`, пока объект не передан в [`return_to_producer`](Self::return_to_producer)
/// или не утёк (`Box::leak`). Лизинг запоминает указатель один раз при приёме кадра и опирается
/// на эту гарантию, отдавая его потребителям через [`crate::RawFrameLease::hardware_buffer`].
pub unsafe trait ProducerFrame: Send {
    /// Буфер кадра. Вызывается один раз при приёме кадра.
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_>;

    /// Вернуть буфер производителю. Буфер свободен для камеры, когда просигналили **все**
    /// fence-ы из `release` (пусто — свободен сразу). Не блокирует: ожидание fence-ов — забота
    /// ядра/HAL (`AImage_deleteAsync`).
    fn return_to_producer(self: Box<Self>, release: ReleaseFences);
}

/// Fence-ы освобождения от всех потребителей кадра. Уже сигнальные сюда не попадают:
/// ждать их нечего.
#[derive(Debug, Default)]
pub struct ReleaseFences {
    pending: Vec<GpuReleaseFence>,
}

impl ReleaseFences {
    /// Добавляет fence; уже сигнальный просто отбрасывается.
    pub fn push(&mut self, fence: GpuReleaseFence) {
        if !fence.is_already_signaled() {
            self.pending.push(fence);
        }
    }

    /// Нечего ждать: буфер можно отдать камере сразу.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Fence-ы во владение производителю (он сливает их в один через `sync_merge`).
    pub fn into_vec(self) -> Vec<GpuReleaseFence> {
        self.pending
    }
}
