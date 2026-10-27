//! Отложенные возвраты: всё, что надо вернуть производителю, но нельзя делать под замком хаба.

use crate::lease::RawFrameLease;
use crate::producer_frame::{ProducerFrame, ReleaseFences};

/// Копилка возвратов, которую хаб собирает под замком и выполняет уже после его отпускания.
///
/// Причина: возврат буфера камере берёт замок `AImageReader` (`AImage_deleteAsync`), а колбэки
/// камеры в этот момент могут звать нас, держа тот же замок. Держать замок хаба поверх замка
/// камеры значит рисковать взаимной блокировкой — поэтому под замком мы только решаем,
/// что вернуть, а возвращаем после.
#[must_use = "возвраты надо выполнить: finish() после отпускания замка хаба"]
#[derive(Default)]
pub(super) struct Deferred {
    /// Лизинги, которых GPU не касался (вытеснены, сброшены при смене эпохи).
    unused_leases: Vec<RawFrameLease>,
    /// Кадры, которые не успели стать лизингом (чужая эпоха, остановка потока).
    untouched_frames: Vec<Box<dyn ProducerFrame>>,
}

impl Deferred {
    pub(super) fn release_unused_later(&mut self, lease: RawFrameLease) {
        self.unused_leases.push(lease);
    }

    pub(super) fn return_untouched_later(&mut self, frame: Box<dyn ProducerFrame>) {
        self.untouched_frames.push(frame);
    }

    /// Выполняет все возвраты. Зовётся строго без замка хаба.
    pub(super) fn finish(self) {
        for lease in self.unused_leases {
            lease.release_unused();
        }
        for frame in self.untouched_frames {
            // GPU кадр не видел: fence-ов нет, буфер свободен сразу.
            frame.return_to_producer(ReleaseFences::default());
        }
    }
}
