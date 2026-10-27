//! Опустошение очереди кадров `AImageReader` под воротами колбэка (нейтральная часть слушателя).
//!
//! Слушатель NDK (`android::reader_callback`) вызывает [`drain_available_frames`] с реальными
//! операциями NDK и реальным производителем потока; тесты на ПК — с поддельными. Так порядок
//! «ворота → забрать → предложить потоку» и гонка с закрытием проверяются без телефона.
//!
//! # Что делает проход
//! Пока reader отдаёт кадры, каждый забирается (`acquire`), при включённой пробе CPU-доступа
//! читается через CPU и предлагается потоку (`offer_frame`) с эпохой этого reader-а. Дальше кадр
//! принадлежит потоку: вернуть его камере он должен сам (`ProducerFrame::return_to_producer`).
//! Проход останавливается, когда:
//! * буферов больше нет (штатно);
//! * reader ответил «все `maxImages` на руках» — это исчерпание пула: оно считается, потоку
//!   сообщается один раз за проход, а кадр остаётся в очереди reader-а;
//! * платформа вернула другой сбой — он считается и логируется (молча не глотается).

use std::time::Instant;

use camera_contract::capture_result::SensorTimestamp;
use raw_frame_stream::{OfferOutcome, OfferedFrame, ProducerFrame, RawFrameProducer, StreamEpoch};

use crate::callback_gate::{CallbackGate, GateAdmission};
use crate::cpu_access_probe::CpuAccessProbe;
use crate::diagnostics::BackendDiagnostics;

const LOG_TARGET: &str = "fasticamera::camera::reader";

/// Итог одной попытки забрать кадр.
pub enum AcquireOutcome {
    /// Кадр получен: образ, метка сенсора и acquire-fence уже упакованы для потока. Образ
    /// возвращается камере только через `ProducerFrame::return_to_producer`.
    Frame(OfferedFrame),
    /// Буферов больше нет (штатный конец очереди).
    NoBuffer,
    /// Все `maxImages` на руках (`AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED`): кадр ждёт в очереди reader-а.
    PoolExhausted,
    /// Сбой платформы с кодом статуса.
    Failed { status: i32 },
}

/// Операции над reader-ом, которыми пользуется слушатель.
pub trait ReaderOps {
    /// `AImageReader_acquireNextImageAsync` + метка + буфер (берёт внутренний замок reader-а).
    fn acquire_next_frame(&self) -> AcquireOutcome;

    /// Отладочная проба: настоящая CPU-копия первой строки кадра (`lock` → копия → `unlock`).
    /// `Ok` — сколько байт скопировано; `Err` — статус отказа `lock`/`unlock`.
    fn copy_first_row(&self, frame: &dyn ProducerFrame) -> Result<usize, i32>;
}

/// Часы `CLOCK_BOOTTIME` — той же шкалы, что `SENSOR_TIMESTAMP` при `TIMESTAMP_SOURCE_REALTIME`.
/// Нужны, чтобы измерить возраст кадра в момент прихода в reader.
pub trait BoottimeClock: Send + Sync {
    /// Текущее время; `None` — часы не прочитались (сбой считается, а не глотается).
    fn now(&self) -> Option<SensorTimestamp>;
}

/// Что слушателю нужно от потока кадров (узкая часть [`RawFrameProducer`]).
pub trait FrameOffers {
    fn offer_frame(&self, epoch: StreamEpoch, frame: OfferedFrame, now: Instant) -> OfferOutcome;
    fn note_pool_exhausted(&self, epoch: StreamEpoch);
    fn note_cpu_copy(&self);
}

impl FrameOffers for RawFrameProducer {
    fn offer_frame(&self, epoch: StreamEpoch, frame: OfferedFrame, now: Instant) -> OfferOutcome {
        RawFrameProducer::offer_frame(self, epoch, frame, now)
    }

    fn note_pool_exhausted(&self, epoch: StreamEpoch) {
        RawFrameProducer::note_pool_exhausted(self, epoch);
    }

    fn note_cpu_copy(&self) {
        RawFrameProducer::note_cpu_copy(self);
    }
}

/// Чем закончился проход.
#[derive(Debug, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Ворота закрыты: reader закрывается, в NDK не заходили.
    GateClosed,
    /// Очередь опустошена; сколько кадров предложено потоку.
    Drained { frames: u64 },
    /// Пул reader-а исчерпан; сколько кадров успели предложить до этого.
    PoolExhausted { frames: u64 },
    /// Платформа вернула сбой; сколько кадров успели предложить до него.
    AcquireFailed { status: i32, frames: u64 },
}

/// Всё, с чем работает один проход (собрано в структуру, чтобы вызов читался по именам).
pub struct DrainPorts<'a, Ops, Offers> {
    pub gate: &'a CallbackGate,
    pub diagnostics: &'a BackendDiagnostics,
    pub ops: &'a Ops,
    pub offers: &'a Offers,
    /// Эпоха потока, которой принадлежит этот reader.
    pub epoch: StreamEpoch,
    pub probe: &'a CpuAccessProbe,
    /// Часы для замера возраста кадра (`now − SENSOR_TIMESTAMP`) при приходе в reader.
    pub clock: &'a dyn BoottimeClock,
}

/// Забирает все доступные кадры и предлагает их потоку, удерживая ворота на всё время работы с NDK.
pub fn drain_available_frames<Ops: ReaderOps, Offers: FrameOffers>(
    ports: &DrainPorts<'_, Ops, Offers>,
) -> DrainOutcome {
    let GateAdmission::Admitted(_pass) = ports.gate.admit() else {
        ports.diagnostics.record_callback_after_close();
        return DrainOutcome::GateClosed;
    };
    let counters = ports.diagnostics.reader();
    let mut frames = 0;
    loop {
        match ports.ops.acquire_next_frame() {
            AcquireOutcome::NoBuffer => return DrainOutcome::Drained { frames },
            AcquireOutcome::PoolExhausted => {
                counters.record_pool_exhausted();
                ports.offers.note_pool_exhausted(ports.epoch);
                tracing::warn!(target: LOG_TARGET, frames, "пул reader-а исчерпан: кадр остался в очереди reader-а");
                return DrainOutcome::PoolExhausted { frames };
            }
            AcquireOutcome::Failed { status } => {
                counters.record_acquire_error(status);
                tracing::error!(target: LOG_TARGET, status, "AImageReader не отдал кадр");
                return DrainOutcome::AcquireFailed { status, frames };
            }
            AcquireOutcome::Frame(frame) => {
                counters.record_frame_acquired();
                record_arrival_age(ports, frame.sensor_timestamp);
                run_cpu_probe_if_armed(ports, &frame);
                let timestamp: SensorTimestamp = frame.sensor_timestamp;
                let outcome = ports.offers.offer_frame(ports.epoch, frame, Instant::now());
                tracing::trace!(target: LOG_TARGET, ?timestamp, ?outcome, "кадр предложен потоку");
                frames += 1;
            }
        }
    }
}

/// Замеряет возраст кадра: `CLOCK_BOOTTIME сейчас − метка сенсора`. Сбой часов и метка из будущего
/// считаются отдельными счётчиками и выборки не дают.
fn record_arrival_age<Ops, Offers>(ports: &DrainPorts<'_, Ops, Offers>, sensor_timestamp: SensorTimestamp) {
    let counters = ports.diagnostics.reader();
    let Some(now) = ports.clock.now() else {
        counters.record_arrival_clock_failure();
        tracing::warn!(target: LOG_TARGET, "часы CLOCK_BOOTTIME не прочитались: возраст кадра не измерен");
        return;
    };
    match now.duration_since(sensor_timestamp) {
        Some(age) => counters.record_arrival_age(age),
        None => {
            counters.record_arrival_age_negative();
            tracing::warn!(target: LOG_TARGET, ?sensor_timestamp, ?now, "метка сенсора позже текущего времени");
        }
    }
}

/// Если проба включена — читает первую строку кадра через CPU и сообщает потоку о копии.
/// Любой отказ считается и логируется.
fn run_cpu_probe_if_armed<Ops: ReaderOps, Offers: FrameOffers>(
    ports: &DrainPorts<'_, Ops, Offers>,
    frame: &OfferedFrame,
) {
    if !ports.probe.is_armed() {
        return;
    }
    let counters = ports.diagnostics.reader();
    // `lock` без fence читает буфер сразу, поэтому проба допустима только на готовом кадре
    // (HAL отдаёт acquire-fence уже сигнальным, факт 00 C4). Иначе честный отказ, а не гонка.
    if !frame.acquire_fence.is_already_signaled() {
        counters.record_cpu_probe_failure();
        tracing::warn!(target: LOG_TARGET, "проба CPU-доступа пропущена: acquire-fence кадра ещё не сигнален");
        return;
    }
    match ports.ops.copy_first_row(frame.frame.as_ref()) {
        Ok(bytes) => {
            counters.record_cpu_probe_copy();
            ports.offers.note_cpu_copy();
            tracing::debug!(target: LOG_TARGET, bytes, "проба CPU-доступа скопировала первую строку кадра");
        }
        Err(status) => {
            counters.record_cpu_probe_failure();
            tracing::warn!(target: LOG_TARGET, status, "проба CPU-доступа: чтение буфера не удалось");
        }
    }
}

#[cfg(test)]
#[path = "reader_drain_tests.rs"]
mod tests;
