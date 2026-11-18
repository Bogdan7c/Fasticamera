//! Граница исполнителя замера AE (решение 68): «отправить статистику кадра — забрать замер позже».
//!
//! Та же форма, что у [`WhiteBalanceEstimator`](crate::executor::WhiteBalanceEstimator):
//! вызывающий (поток рендера) знает только [`ExposureMeter`]. Сегодня за ним эталон на CPU
//! ([`CpuExposureMeter`]), завтра — compute-проход с чтением после fence слота, и соседи не меняются.

use std::collections::VecDeque;

use camera_contract::auto_exposure::{ExposureProduct, MeteredArea};
use isp_model::exposure::LuminanceWeights;
use isp_model::statistics::AwbStatistics;

use super::config::MeteringConfig;
use super::measurement::{ExposureMeasurement, ExposureMeasurementError, ExposureTarget, measure_exposure};
use super::metering_weights::MeteringWeights;
use crate::error::ConfigError;
use crate::executor::FrameTag;
pub use crate::executor::SubmitError;

/// Запрос замера: статистика кадра и всё, что нужно, чтобы превратить её в «сколько света».
#[derive(Debug, Clone, Copy)]
pub struct ExposureMeteringRequest<'a> {
    /// Чей кадр.
    pub frame: FrameTag,
    /// Область, под которую построена таблица весов (возвращается с ответом: по ней контроллер
    /// отсеивает замеры чужой области).
    pub area: MeteredArea,
    /// Плитки статистики кадра.
    pub statistics: &'a AwbStatistics,
    /// Веса плиток этого потока и области.
    pub weights: &'a MeteringWeights,
    /// Веса яркости (`IspParams::scene_luminance_weights` кадра).
    pub luminance: LuminanceWeights,
    /// «Выдержка × ISO» этого кадра из его результата (`FrameCaptureMeta::Paired`); подставлять нельзя.
    pub frame_product: ExposureProduct,
    /// Цель яркости (`scene_middle_gray` тона).
    pub target: ExposureTarget,
}

/// Готовый замер кадра.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedExposureMeasurement {
    /// Чей кадр.
    pub frame: FrameTag,
    /// Область замера из запроса.
    pub area: MeteredArea,
    /// «Выдержка × ISO» кадра (нужна для цели показа и отладки).
    pub frame_product: ExposureProduct,
    /// Результат или причина, почему замера нет.
    pub outcome: Result<ExposureMeasurement, ExposureMeasurementError>,
}

/// Исполнитель замера экспозиции.
pub trait ExposureMeter {
    /// Принимает кадр статистики. `Ok` — ответ придёт через [`Self::poll`].
    fn submit(&mut self, request: ExposureMeteringRequest<'_>) -> Result<(), SubmitError>;

    /// Забирает следующий готовый замер (самый старый); `None` — готовых нет.
    fn poll(&mut self) -> Option<CompletedExposureMeasurement>;
}

/// Сколько готовых замеров CPU-исполнитель держит, если их не забирают.
const CPU_COMPLETED_CAPACITY: usize = 4;

/// Эталон на CPU: считает в `submit`, ответ отдаёт в `poll` тут же.
#[derive(Debug, Clone)]
pub struct CpuExposureMeter {
    config: MeteringConfig,
    completed: VecDeque<CompletedExposureMeasurement>,
    overwritten: u64,
}

impl CpuExposureMeter {
    /// Исполнитель с проверенным конфигом.
    pub fn new(config: MeteringConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            completed: VecDeque::new(),
            overwritten: 0,
        })
    }

    /// Сколько готовых замеров вытеснено, потому что их не забирали (в штатной работе 0).
    pub fn overwritten(&self) -> u64 {
        self.overwritten
    }
}

impl ExposureMeter for CpuExposureMeter {
    fn submit(&mut self, request: ExposureMeteringRequest<'_>) -> Result<(), SubmitError> {
        let outcome = measure_exposure(
            request.statistics,
            request.weights,
            request.luminance,
            request.frame_product,
            request.target,
            &self.config,
        );
        if self.completed.len() >= CPU_COMPLETED_CAPACITY {
            self.completed.pop_front();
            self.overwritten += 1;
        }
        self.completed.push_back(CompletedExposureMeasurement {
            frame: request.frame,
            area: request.area,
            frame_product: request.frame_product,
            outcome,
        });
        Ok(())
    }

    fn poll(&mut self) -> Option<CompletedExposureMeasurement> {
        self.completed.pop_front()
    }
}

#[cfg(test)]
#[path = "meter_tests.rs"]
mod tests;
