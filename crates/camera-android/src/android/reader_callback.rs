//! Слушатель `AImageReader` с лизингом кадров. Safety-модуль №2 из двух, где разрешён `extern "C"`.
//!
//! Слушатель забирает кадры (`AImageReader_acquireNextImageAsync`), упаковывает каждый в
//! [`AndroidProducerFrame`] и предлагает потоку кадров ([`RawFrameProducer::offer_frame`]) с эпохой
//! своего reader-а. Дальше кадром владеет поток: пиксели через CPU не проходят (кроме отладочной
//! пробы), а образ возвращается камере тем, кто последним закончил чтение.
//!
//! Колбэк идёт на потоке looper-а самого `AImageReader`, не ждёт ничего, что держит поток камеры
//! или поток рендера (все вызовы потока кадров — короткие замки и счётчики), и не паникует через
//! границу C (под `catch_unwind`). Порядок «ворота → забрать → предложить» и гонка с закрытием
//! живут в нейтральных `reader_drain` и `callback_gate` (там же тесты).

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::raw_format::RawFormat;
use camera_ndk_sys as ndk;
use raw_frame_stream::{OfferedFrame, ProducerFrame, RawFrameProducer, StreamEpoch};
use shell_contract::sync_fence::GpuWaitFence;

use crate::callback_gate::CallbackGate;
use crate::cpu_access_probe::{CpuAccessProbe, first_row_bytes};
use crate::diagnostics::BackendDiagnostics;
use crate::reader_drain::{AcquireOutcome, DrainPorts, ReaderOps, drain_available_frames};
use crate::tags;

use super::boottime_clock::SystemBoottimeClock;
use super::producer_frame::{AndroidProducerFrame, FrameReturnPath};

const LOG_TARGET: &str = "fasticamera::camera::reader";

/// Контекст слушателя. Принадлежит `NdkReader` и освобождается после `AImageReader_delete`.
///
/// Два отдельных факта о времени жизни (оба по `media/ndk/NdkImageReader.cpp`, AOSP main):
/// * деструктор reader-а останавливает свой looper с ожиданием, поэтому после возврата
///   `AImageReader_delete` колбэк уже не придёт и контекст можно освобождать;
/// * но сам `delete` (`AImageReader::close()`, строки ~380–397) ждёт остановки looper-а, **удерживая
///   внутренний замок reader-а**, а колбэк внутри `acquireNextImage*` ждёт тот же замок.
///   Поэтому перед `delete` обязательно закрываются ворота ([`ReaderContext::stop`]): после
///   их возврата ни один колбэк не внутри NDK, а новые в NDK не заходят.
///
/// Остановка ([`ReaderContext::stop`]) происходит при закрытии сессии, а удаление reader-а — позже,
/// когда все кадры вернулись (см. `retired_readers`).
pub(super) struct ReaderContext {
    gate: CallbackGate,
    /// Счёт невозвращённых кадров и диагностика; кадры держат свои клоны.
    path: Arc<FrameReturnPath>,
    producer: RawFrameProducer,
    epoch: StreamEpoch,
    probe: CpuAccessProbe,
    raw_format: RawFormat,
    /// Остановка уже выполнена: `end_stream` зовётся ровно один раз на эпоху.
    stopped: AtomicBool,
}

impl ReaderContext {
    pub(super) fn new(
        diagnostics: Arc<BackendDiagnostics>,
        producer: RawFrameProducer,
        epoch: StreamEpoch,
        probe: CpuAccessProbe,
        raw_format: RawFormat,
    ) -> Self {
        Self {
            gate: CallbackGate::new(),
            path: Arc::new(FrameReturnPath::new(diagnostics)),
            producer,
            epoch,
            probe,
            raw_format,
            stopped: AtomicBool::new(false),
        }
    }

    /// Сколько кадров этого reader-а ещё не возвращено камере.
    pub(super) fn outstanding_frames(&self) -> u64 {
        self.path.outstanding.count()
    }

    /// Прекращает приём кадров. Идемпотентно. Порядок важен:
    /// 1. `end_stream`/`end_standby_stream` — поток отпускает кадры, которые держит сам, и считает эту эпоху устаревшей
    ///    (кадр, предложенный между шагами 1 и 2, сразу вернётся как `StaleEpoch`);
    /// 2. закрытие ворот — ждёт колбэк, уже вошедший в NDK, и не пускает новые.
    ///
    /// После возврата новых кадров из этого reader-а не будет, число невозвращённых только убывает.
    pub(super) fn stop(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        // Роль эпохи меняется обменом (13b): закрываем обоими способами, лишний — no-op хаба.
        self.producer.end_stream(self.epoch);
        self.producer.end_standby_stream(self.epoch);
        self.gate.close();
    }

    pub(super) fn as_callback_context(self: &Arc<Self>) -> *mut c_void {
        Arc::as_ptr(self).cast_mut().cast()
    }
}

/// Операции NDK над живым reader-ом для нейтрального опустошения очереди.
struct NdkReaderOps<'a> {
    reader: *mut ndk::AImageReader,
    context: &'a ReaderContext,
}

impl NdkReaderOps<'_> {
    /// Образ получен, но упаковать его не удалось: отдаём камере без чтения и сообщаем отказ.
    fn discard_image(&self, image: NonNull<ndk::AImage>, status: i32) -> AcquireOutcome {
        // SAFETY: образ только что получен `acquire` и нигде не сохранён; `AImage_delete` — штатный
        // возврат нетронутого образа (fence не нужен: GPU его не видел).
        unsafe { ndk::AImage_delete(image.as_ptr()) };
        AcquireOutcome::Failed { status }
    }

    /// Метка сенсора и буфер образа; `Err` — статус отказа NDK.
    fn describe_image(image: NonNull<ndk::AImage>) -> Result<(SensorTimestamp, NonNull<c_void>), i32> {
        let mut nanos: i64 = 0;
        // SAFETY: образ жив (мы его владелец), `nanos` — валидное выходное место.
        let status = unsafe { ndk::AImage_getTimestamp(image.as_ptr(), &mut nanos) };
        if status != tags::MEDIA_STATUS_OK {
            return Err(status);
        }
        let mut buffer: *mut ndk::AHardwareBuffer = ptr::null_mut();
        // SAFETY: образ жив, `buffer` — валидное выходное место; буфер принадлежит образу.
        let status = unsafe { ndk::AImage_getHardwareBuffer(image.as_ptr(), &mut buffer) };
        if status != tags::MEDIA_STATUS_OK {
            return Err(status);
        }
        let buffer = NonNull::new(buffer.cast::<c_void>()).ok_or(tags::MEDIA_STATUS_UNKNOWN_ERROR)?;
        Ok((SensorTimestamp::from_nanos(nanos), buffer))
    }
}

impl ReaderOps for NdkReaderOps<'_> {
    fn acquire_next_frame(&self) -> AcquireOutcome {
        let mut image: *mut ndk::AImage = ptr::null_mut();
        let mut acquire_fence_fd: i32 = -1;
        // SAFETY: reader жив на время колбэка (`delete` не начнётся, пока ворота удерживаются
        // колбэком, см. `ReaderContext::stop`); `image` и `acquire_fence_fd` — валидные выходные места.
        let status = unsafe { ndk::AImageReader_acquireNextImageAsync(self.reader, &mut image, &mut acquire_fence_fd) };
        if status == tags::MEDIA_STATUS_NO_BUFFER_AVAILABLE {
            return AcquireOutcome::NoBuffer;
        }
        if status == tags::MEDIA_STATUS_MAX_IMAGES_ACQUIRED {
            return AcquireOutcome::PoolExhausted;
        }
        let Some(image) = NonNull::new(image).filter(|_| status == tags::MEDIA_STATUS_OK) else {
            let status = if status == tags::MEDIA_STATUS_OK {
                tags::MEDIA_STATUS_UNKNOWN_ERROR
            } else {
                status
            };
            return AcquireOutcome::Failed { status };
        };

        // Fence берём во владение сразу: любой выход ниже закрывает дескриптор через `Drop`.
        // SAFETY: при OK `acquire_fence_fd` — открытый дескриптор (или -1), который NDK передал нам
        // и который больше никто не закроет.
        let acquire_fence = match unsafe { GpuWaitFence::from_raw_ndk_fd(acquire_fence_fd) } {
            Ok(fence) => fence,
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, acquire_fence_fd, "acquire-fence непригоден");
                return self.discard_image(image, tags::MEDIA_STATUS_UNKNOWN_ERROR);
            }
        };
        let (sensor_timestamp, buffer) = match Self::describe_image(image) {
            Ok(described) => described,
            Err(status) => return self.discard_image(image, status),
        };
        // SAFETY: `image` получен `acquire` и ещё не освобождён; `buffer` — `AImage_getHardwareBuffer`
        // этого же образа.
        let frame = unsafe { AndroidProducerFrame::adopt(image, buffer, Arc::clone(&self.context.path)) };
        AcquireOutcome::Frame(OfferedFrame {
            sensor_timestamp,
            acquire_fence,
            frame: Box::new(frame),
        })
    }

    fn copy_first_row(&self, frame: &dyn ProducerFrame) -> Result<usize, i32> {
        let buffer = frame.hardware_buffer().as_ptr().as_ptr().cast::<ndk::AHardwareBuffer>();
        let mut description = ndk::AHardwareBuffer_Desc::default();
        // SAFETY: буфер жив (его держит образ кадра, который у нас в руках); `description` — валидное место.
        unsafe { ndk::AHardwareBuffer_describe(buffer, &mut description) };
        let row_bytes = first_row_bytes(self.context.raw_format, description.stride, usize::MAX);
        if row_bytes == 0 {
            return Err(tags::MEDIA_STATUS_UNKNOWN_ERROR);
        }

        let mut address: *mut c_void = ptr::null_mut();
        // SAFETY: буфер жив; fence -1 допустим, потому что вызывающий пропускает только кадры с уже
        // сигнальным acquire-fence; прямоугольник null = весь буфер; `address` — валидное место.
        let status =
            unsafe { ndk::AHardwareBuffer_lock(buffer, tags::USAGE_CPU_READ_RARELY, -1, ptr::null(), &mut address) };
        if status != 0 {
            return Err(status);
        }
        let mut row = vec![0_u8; row_bytes];
        if !address.is_null() {
            // SAFETY: буфер заблокирован для чтения, первая строка занимает `stride` единиц формата
            // (`first_row_bytes` считает байты по формату), приёмник — свежий `Vec` нужного размера.
            unsafe { ptr::copy_nonoverlapping(address.cast::<u8>(), row.as_mut_ptr(), row_bytes) };
        }
        // SAFETY: буфер заблокирован нами выше; fence не нужен (ждём синхронно, `null`).
        let unlock_status = unsafe { ndk::AHardwareBuffer_unlock(buffer, ptr::null_mut()) };
        if address.is_null() {
            return Err(tags::MEDIA_STATUS_UNKNOWN_ERROR);
        }
        if unlock_status != 0 {
            return Err(unlock_status);
        }
        // Копия должна реально случиться: оптимизатор не вправе выкинуть её как мёртвую запись.
        Ok(std::hint::black_box(row).len())
    }
}

/// `AImageReader_ImageListener::onImageAvailable`.
pub(super) unsafe extern "C" fn reader_on_image_available(context: *mut c_void, reader: *mut ndk::AImageReader) {
    // SAFETY: context — `ReaderContext` из `NdkReader`, живой до конца `AImageReader_delete`.
    let Some(context) = (unsafe { context.cast_const().cast::<ReaderContext>().as_ref() }) else {
        return;
    };
    let ops = NdkReaderOps { reader, context };
    let drained = catch_unwind(AssertUnwindSafe(|| {
        drain_available_frames(&DrainPorts {
            gate: &context.gate,
            diagnostics: &context.path.diagnostics,
            ops: &ops,
            offers: &context.producer,
            epoch: context.epoch,
            probe: &context.probe,
            clock: &SystemBoottimeClock,
        })
    }));
    if drained.is_err() {
        context.path.diagnostics.record_callback_panic();
        tracing::error!(target: LOG_TARGET, "паника в слушателе AImageReader перехвачена");
    }
}
