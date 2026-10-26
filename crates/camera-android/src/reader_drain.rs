//! Опустошение очереди кадров `AImageReader` под воротами колбэка (нейтральная часть слушателя).
//!
//! Слушатель NDK ([`crate::callback_gate`], `android::reader_callback`) вызывает [`drain_available_images`]
//! с реальными операциями NDK; тесты на ПК — с поддельными. Так порядок «ворота → забрать → отпустить»
//! и гонка с закрытием проверяются без телефона.

use crate::callback_gate::{CallbackGate, GateAdmission};
use crate::diagnostics::BackendDiagnostics;

/// Итог одной попытки забрать кадр.
#[derive(Debug)]
pub enum AcquireOutcome<Image> {
    /// Кадр получен; его нужно вернуть через [`ReaderOps::release_image`].
    Image(Image),
    /// Буферов больше нет (штатный конец очереди).
    NoBuffer,
    /// Сбой платформы с кодом статуса.
    Failed { status: i32 },
}

/// Операции над reader-ом, которыми пользуется слушатель.
pub trait ReaderOps {
    type Image;

    /// `AImageReader_acquireNextImage` (берёт внутренний замок reader-а).
    fn acquire_next_image(&self) -> AcquireOutcome<Self::Image>;

    /// `AImage_delete`: вернуть буфер reader-у.
    fn release_image(&self, image: Self::Image);
}

/// Чем закончилось опустошение.
#[derive(Debug, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Ворота закрыты: reader удаляется, в NDK не заходили.
    GateClosed,
    /// Очередь опустошена; сколько кадров забрано и отпущено.
    Drained { frames: u64 },
    /// Платформа вернула сбой; сколько кадров успели забрать до него.
    AcquireFailed { status: i32, frames: u64 },
}

/// Забирает и сразу отпускает все доступные кадры, удерживая ворота на всё время работы с NDK.
pub fn drain_available_images<Ops: ReaderOps>(
    gate: &CallbackGate,
    diagnostics: &BackendDiagnostics,
    ops: &Ops,
) -> DrainOutcome {
    let GateAdmission::Admitted(_pass) = gate.admit() else {
        diagnostics.record_callback_after_close();
        return DrainOutcome::GateClosed;
    };
    let counters = diagnostics.raw_sink();
    let mut frames = 0;
    loop {
        match ops.acquire_next_image() {
            AcquireOutcome::NoBuffer => return DrainOutcome::Drained { frames },
            AcquireOutcome::Failed { status } => {
                counters.record_acquire_error(status);
                return DrainOutcome::AcquireFailed { status, frames };
            }
            AcquireOutcome::Image(image) => {
                counters.record_frame_acquired();
                ops.release_image(image);
                counters.record_frame_released();
                frames += 1;
            }
        }
    }
}

#[cfg(test)]
#[path = "reader_drain_tests.rs"]
mod tests;
