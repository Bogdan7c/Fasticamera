//! Кадр камеры, отданный потоку: `AImage` с буфером и путь его возврата камере.
//!
//! Владение (инвариант 2 README сессии 05): образ `AImage` принадлежит этому объекту с момента
//! `acquire` до [`ProducerFrame::return_to_producer`]. Возврат — `AImage_deleteAsync(image, fd)`:
//! буфер вернётся в очередь reader-а, когда сигнален fence `fd` (слитый из fence-ов всех
//! потребителей). Если слить fence-ы не удалось, образ **намеренно не возвращается** (утечка со
//! счётчиком, решение 43): GPU мог ещё читать буфер, а вернуть его камере с неполным fence —
//! значит позволить HAL писать поверх читаемого.

use std::ffi::{CStr, c_void};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::Arc;

use camera_ndk_sys as ndk;
use raw_frame_stream::{ProducerFrame, ReleaseFences};
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;

use crate::diagnostics::BackendDiagnostics;
use crate::fence_fold::{FenceMerger, FoldOutcome, fold_fences};
use crate::outstanding_frames::OutstandingFrames;

const LOG_TARGET: &str = "fasticamera::camera::reader";

/// Имя для отладки в ядре (`/sys/kernel/debug/sync`), которое получает слитый fence.
const MERGED_FENCE_NAME: &CStr = c"fasticamera_release";

/// Общее для всех кадров одного reader-а: счёт невозвращённых и диагностика.
pub(super) struct FrameReturnPath {
    pub(super) outstanding: OutstandingFrames,
    pub(super) diagnostics: Arc<BackendDiagnostics>,
}

impl FrameReturnPath {
    pub(super) fn new(diagnostics: Arc<BackendDiagnostics>) -> Self {
        Self {
            outstanding: OutstandingFrames::new(),
            diagnostics,
        }
    }
}

/// `sync_merge` из libsync (API 26).
struct NdkFenceMerger;

impl FenceMerger for NdkFenceMerger {
    type Fence = OwnedFd;
    /// Код ошибки `sync_merge` (отрицательный errno).
    type Error = i32;

    fn merge(&self, first: &OwnedFd, second: &OwnedFd) -> Result<OwnedFd, i32> {
        // SAFETY: оба дескриптора открыты и принадлежат заимствованным `OwnedFd`; `sync_merge` их
        // не закрывает (входные остаются валидными), имя — NUL-терминированная строка 'static.
        let merged = unsafe { ndk::sync_merge(MERGED_FENCE_NAME.as_ptr(), first.as_raw_fd(), second.as_raw_fd()) };
        if merged < 0 {
            return Err(merged);
        }
        // SAFETY: `sync_merge` вернул новый открытый дескриптор, которого больше никто не держит.
        Ok(unsafe { OwnedFd::from_raw_fd(merged) })
    }
}

/// Что делать с образом после попытки слить fence-ы.
enum ReleasePlan {
    /// Вернуть камере сразу (`-1`).
    ReturnNow,
    /// Вернуть, когда сигнален этот fence (владение им уходит NDK).
    ReturnAfter(OwnedFd),
    /// Слияние не удалось: образ не возвращать.
    Leak { failed_merge: usize, status: i32 },
}

/// Планирует возврат: сливает fence-ы потребителей в один.
fn plan_release(release: ReleaseFences) -> ReleasePlan {
    // `ReleaseFences` хранит только несигнальные fence-ы; `into_owned_fd` страхует от сигнального.
    let descriptors: Vec<OwnedFd> = release
        .into_vec()
        .into_iter()
        .filter_map(shell_contract::sync_fence::GpuReleaseFence::into_owned_fd)
        .collect();
    match fold_fences(&NdkFenceMerger, descriptors) {
        FoldOutcome::NothingToWait => ReleasePlan::ReturnNow,
        FoldOutcome::Merged(fence) => ReleasePlan::ReturnAfter(fence),
        FoldOutcome::Failed { failed_merge, error } => ReleasePlan::Leak {
            failed_merge,
            status: error,
        },
    }
}

/// Где образ находится в своём жизненном цикле.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameState {
    /// На руках: ждёт возврата.
    Held,
    /// Возвращён камере (`AImage_deleteAsync`).
    Returned,
    /// Не возвращён намеренно и посчитан как утечка.
    Leaked,
}

/// `AImage` как кадр потока.
pub(super) struct AndroidProducerFrame {
    image: NonNull<ndk::AImage>,
    buffer: NonNull<c_void>,
    path: Arc<FrameReturnPath>,
    state: FrameState,
}

// SAFETY: `AImage` — потокобезопасный объект NDK (его методы берут внутренний замок reader-а), а
// `AImage_deleteAsync` по документации допустим с любого потока; именно поэтому кадр и уезжает на
// поток рендера. Указатель на буфер — `AHardwareBuffer*` с атомарным счётчиком ссылок.
unsafe impl Send for AndroidProducerFrame {}

impl AndroidProducerFrame {
    /// Принимает образ в владение и ставит его на учёт невозвращённых.
    ///
    /// # Safety
    /// `image` — образ, полученный `AImageReader_acquireNextImageAsync` и ещё не освобождённый;
    /// `buffer` — `AImage_getHardwareBuffer(image)` этого же образа (живёт, пока жив образ).
    pub(super) unsafe fn adopt(
        image: NonNull<ndk::AImage>,
        buffer: NonNull<c_void>,
        path: Arc<FrameReturnPath>,
    ) -> Self {
        path.outstanding.frame_acquired();
        Self {
            image,
            buffer,
            path,
            state: FrameState::Held,
        }
    }

    /// Возвращает образ камере с fence-ом освобождения (`-1` — без ожидания).
    fn return_with_fence(&mut self, release_fence_fd: i32) {
        // SAFETY: образ на руках у нас (состояние `Held`), второй раз его не освободят; fd либо -1,
        // либо открытый дескриптор, владение которым переходит NDK.
        unsafe { ndk::AImage_deleteAsync(self.image.as_ptr(), release_fence_fd) };
        self.state = FrameState::Returned;
        self.path.diagnostics.reader().record_image_returned();
        // Счёт уменьшается строго после возврата: reader удаляется только при нуле, а значит
        // `AImageReader_delete` никогда не пересекается с `AImage_deleteAsync`.
        self.path.outstanding.frame_returned();
    }

    /// Образ остаётся у нас навсегда; reader поэтому тоже не будет удалён (счёт не уменьшается).
    fn leak(&mut self, reason: &'static str) {
        self.state = FrameState::Leaked;
        self.path.diagnostics.reader().record_image_leaked();
        tracing::error!(target: LOG_TARGET, reason, "AImage не возвращён камере и оставлен (утечка)");
    }
}

// SAFETY: `hardware_buffer` — буфер образа, который живёт, пока образ не возвращён камере или не
// утёк; в обоих случаях потребители к этому моменту уже вернули кадр, и объект больше не используется.
unsafe impl ProducerFrame for AndroidProducerFrame {
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        // SAFETY: см. контракт `adopt`: буфер жив, пока жив образ (а образ жив, пока жив `self`).
        unsafe { BorrowedHardwareBuffer::from_raw(self.buffer) }
    }

    fn return_to_producer(mut self: Box<Self>, release: ReleaseFences) {
        match plan_release(release) {
            ReleasePlan::ReturnNow => self.return_with_fence(-1),
            ReleasePlan::ReturnAfter(fence) => self.return_with_fence(fence.into_raw_fd()),
            ReleasePlan::Leak { failed_merge, status } => {
                self.path.diagnostics.reader().record_fence_merge_failure();
                tracing::error!(
                    target: LOG_TARGET,
                    failed_merge,
                    status,
                    "sync_merge не удался: образ не возвращается камере, GPU мог ещё читать буфер"
                );
                self.leak("слияние fence-ов освобождения не удалось");
            }
        }
    }
}

impl Drop for AndroidProducerFrame {
    fn drop(&mut self) {
        // Кадр уничтожили, не вернув производителю: нарушение контракта потока. Образ не
        // освобождаем (GPU мог читать), но и не молчим.
        if self.state == FrameState::Held {
            self.leak("кадр уничтожен без возврата производителю");
        }
    }
}
