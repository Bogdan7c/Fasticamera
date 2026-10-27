//! Счётчик кадров reader-а, которые ещё на руках у потребителей (нейтральная часть).
//!
//! Кадр «на руках» с момента `acquire` до возврата камере (`AImage_deleteAsync`). Пока счётчик
//! не ноль, reader удалять нельзя: `AImageReader_delete` освободил бы память, которую GPU может
//! ещё читать. Кадр, который намеренно не возвращён (утечка), остаётся в счётчике навсегда —
//! и именно поэтому его reader тоже никогда не будет удалён.

use std::sync::atomic::{AtomicU64, Ordering};

/// Число невозвращённых кадров одного reader-а. Потокобезопасен без замков.
#[derive(Debug, Default)]
pub struct OutstandingFrames {
    count: AtomicU64,
}

impl OutstandingFrames {
    pub fn new() -> Self {
        Self::default()
    }

    /// Кадр забран у reader-а.
    pub fn frame_acquired(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    /// Кадр возвращён камере. Счёт не уходит ниже нуля: лишний возврат (ошибка вызывающего)
    /// не должен «занять» чужое место и позволить удалить reader раньше времени.
    pub fn frame_returned(&self) {
        let result = self
            .count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_sub(1));
        if result.is_err() {
            tracing::error!(
                target: "fasticamera::camera::reader",
                "возврат кадра при нуле невозвращённых: счёт кадров нарушен"
            );
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Acquire)
    }
}

#[cfg(test)]
#[path = "outstanding_frames_tests.rs"]
mod tests;
