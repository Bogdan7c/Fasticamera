//! `raw-frame-stream` — лизинг RAW-кадров камеры без копий (сессия 05, решение 40).
//!
//! # Зачем
//! Камера отдаёт ограниченное число буферов (`maxImages` у `AImageReader`). Каждый буфер
//! выдаётся потребителям «под расписку» — [`RawFrameLease`] — и возвращается камере только
//! явно: после fence GPU ([`RawFrameLease::release_after_gpu_fence`]) или как нетронутый
//! ([`RawFrameLease::release_unused`]). Забытый лизинг — утечка со счётчиком, а не возврат
//! (решение 43): мы не знаем, дочитал ли его GPU.
//!
//! # Кто с кем говорит
//! ```text
//! camera-android ──RawFrameProducer──▶ [RawFrameStream] ──ViewfinderFrames──▶ render-loop (раз в vsync)
//!   (кадры + результаты)                  спаривание,     ──SnapshotRingFrames──▶ (сессия 14)
//!                                          раздача,        ──LosslessFrames──▶ (будущая запись)
//!                                          счётчики
//! ```
//! `camera-android` и `render-loop` знают только этот crate и словари, но не друг друга.
//!
//! # Инварианты
//! 1. Кадр раздаётся потребителям только с паспортом: метаданные из результата с **той же**
//!    меткой `SENSOR_TIMESTAMP` или честное «метаданных нет (причина)» (решение 42).
//! 2. Буфер возвращается производителю ровно один раз, когда последний лизинг возвращён;
//!    fence-ы всех потребителей передаются производителю вместе.
//! 3. Ни один вызов не ждёт другой поток: только короткие замки и атомарные счётчики.
//! 4. Каждый сброс посчитан: вытеснение «самым свежим», переполнение буфера видоискателя
//!    (решение 53), вытеснение из кольца, переполнение
//!    очереди «ни одного молча» (брак), исчерпание пула, неспаренные кадры и результаты.

mod config;
mod counters;
mod lease;
mod pool;
mod producer_frame;
mod ring_pick;
mod stream;
mod viewfinder_pacing;

pub use config::{DEFAULT_SNAPSHOT_RING_BUDGET_BYTES, RawFrameStreamConfig, RawFrameStreamConfigError};
pub use counters::RawStreamCounters;
pub use lease::RawFrameLease;
pub use pool::PoolPlan;
pub use producer_frame::{ProducerFrame, ReleaseFences};
pub use ring_pick::{FlashRingPick, RingPick, RingPickRequest, RingSubstitutionReason, RingUnavailableReason};
pub use stream::{
    DefectReason, LosslessFrames, OfferOutcome, OfferedFrame, RawFrameProducer, RawFrameStream, ResultOutcome,
    SnapshotRingFrames, StandbyBegin, StandbyPromotion, StreamDefect, StreamEpoch, StreamStart, ViewfinderFrames,
    ViewfinderPoll,
};
pub use viewfinder_pacing::{JitterBufferConfig, ViewfinderPacing, ViewfinderPacingSwitch};
