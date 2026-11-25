//! Наш AE «в форме шейдера» (сессия 08a, решения 63, 66, 68).
//!
//! Поток рендера — «разведчик»: меряет сцену и предлагает, сколько света нужно
//! ([`ExposureProduct`](camera_contract::auto_exposure::ExposureProduct)). Всё остальное
//! (компенсация, пределы, делёж выдержка/ISO, мерцание) решает движок камеры.
//!
//! # Состав
//! * [`config`] — все числа (serde, `deny_unknown_fields`, `validate()`), каждое — оценка до 08c/09;
//! * [`metering_weights`] — таблица весов плиток для матричного и точечного замера (CPU, один раз на поток и область);
//! * [`measurement`] — замер: плитки статистики → требуемое произведение и качество (только арифметика и редукции);
//! * [`meter`] — граница исполнителя [`ExposureMeter`] (submit / poll) и эталон [`CpuExposureMeter`];
//! * [`controller`] — сглаживание цели, профили, мёртвая зона видео, блокировка, остаточная подтяжка ISP;
//! * [`publication`] — ограничитель частоты публикаций предложения (внутренний).
//!
//! # Последовательность на кадр (поток рендера)
//! 1. `controller.set_directive(..)` — когда движок опубликовал новое указание;
//! 2. кадр статистики (каждый N-й): `meter.submit(request)` с произведением ЭТОГО кадра из его
//!    результата; нет парного результата — `note_skipped_measurement(NoPairedExposure)`;
//! 3. `while let Some(done) = meter.poll() { controller.ingest(&done) }`;
//! 4. `controller.advance(метка_сенсора)`;
//! 5. `controller.proposal(метка_сенсора)` → если `Some`, в ящик предложений;
//! 6. `controller.display_gain(произведение_кадра)` → `ProcessingControls::exposure_gain`.

pub mod config;
pub mod controller;
mod frame_tag;
pub mod linear;
pub mod measurement;
pub mod meter;
pub mod metering_weights;
mod publication;
#[cfg(test)]
#[path = "simulation_tests.rs"]
mod simulation_tests;
#[cfg(test)]
mod test_support;

pub use config::{ExposureControllerConfig, MeteringConfig, SceneAverage};
pub use controller::{AutoExposureController, ExposureCounters, IngestOutcome, SkippedMeasurement};
pub use frame_tag::{FrameTag, SubmitError};
pub use linear::linear_mid_gray_product;
pub use measurement::{
    ExposureMeasurement, ExposureMeasurementError, ExposureTarget, MeasurementPhase, measure_exposure,
    measure_exposure_in_phase,
};
pub use meter::{CompletedExposureMeasurement, CpuExposureMeter, ExposureMeter, ExposureMeteringRequest};
pub use metering_weights::{MeteringArea, MeteringWeights, MeteringWeightsError, TilePlacement};
