//! Граница исполнителя оценки (решение 68): «отправить кадр статистики — забрать оценку позже».
//!
//! Вызывающий (поток рендера) знает только [`WhiteBalanceEstimator`]. Сегодня за ним — эталон на CPU
//! ([`CpuWhiteBalanceEstimator`]), который считает синхронно прямо в `submit`; завтра — compute-проход,
//! который пишет несколько f32 в буфер и отдаёт результат через `poll` после fence слота, и никто из
//! соседей не меняется. Для AE (08a) встанет тот же по форме трейт над той же статистикой.
//!
//! Идентичность кадра ([`FrameTag`]) едет с запросом и возвращается с ответом: потоку важно знать, чья это
//! оценка (кадр мог быть из потока, который уже сменился).

use std::collections::VecDeque;

use isp_model::statistics::AwbStatistics;

use crate::config::EstimatorConfig;
use crate::error::EstimationError;
use crate::estimate::EstimateOutcome;
use crate::estimator::{estimate_scene_light, finalize_estimate};
use crate::locus_table::CameraLocusTable;

/// Чей кадр оценивается. Поля — то, что нужно вызывающему, чтобы привязать ответ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTag {
    /// Поколение потока камеры у вызывающего (оценка чужого потока отбрасывается им же).
    pub stream_generation: u64,
    /// Порядковый номер кадра в потоке.
    pub sequence: u64,
    /// Метка сенсора кадра, наносекунды (шкала BOOTTIME).
    pub sensor_timestamp_nanos: i64,
}

/// Запрос оценки: статистика кадра и таблица локуса ЕГО потока.
#[derive(Debug, Clone, Copy)]
pub struct EstimationRequest<'a> {
    /// Чей кадр.
    pub frame: FrameTag,
    /// Плитки статистики (раскладка — `isp_model::statistics`).
    pub statistics: &'a AwbStatistics,
    /// Таблица локуса камеры этого потока.
    pub table: &'a CameraLocusTable,
}

/// Готовая оценка кадра.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedEstimate {
    /// Чья оценка.
    pub frame: FrameTag,
    /// Исход или причина, почему оценки нет.
    pub outcome: Result<EstimateOutcome, EstimationError>,
}

/// Почему запрос не принят.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    /// Исполнитель занят и не принимает новых запросов (backpressure GPU-реализации): вызывающий
    /// пропускает кадр статистики — следующий придёт через несколько кадров.
    #[error("исполнитель оценки занят")]
    Busy,
}

/// Исполнитель оценки света сцены.
pub trait WhiteBalanceEstimator {
    /// Принимает кадр статистики. `Ok` — запрос принят и ответ придёт через [`Self::poll`].
    fn submit(&mut self, request: EstimationRequest<'_>) -> Result<(), SubmitError>;

    /// Забирает следующую готовую оценку (самую старую); `None` — готовых нет.
    fn poll(&mut self) -> Option<CompletedEstimate>;
}

/// Сколько готовых оценок CPU-исполнитель держит, если их не забирают.
const CPU_COMPLETED_CAPACITY: usize = 4;

/// Эталон на CPU: считает в `submit`, ответ отдаёт в `poll` тут же.
#[derive(Debug, Clone)]
pub struct CpuWhiteBalanceEstimator {
    config: EstimatorConfig,
    completed: VecDeque<CompletedEstimate>,
    overwritten: u64,
}

impl CpuWhiteBalanceEstimator {
    /// Исполнитель с проверенным конфигом.
    pub fn new(config: EstimatorConfig) -> Result<Self, EstimationError> {
        config.validate()?;
        Ok(Self {
            config,
            completed: VecDeque::new(),
            overwritten: 0,
        })
    }

    /// Сколько готовых оценок вытеснено, потому что их не забирали (в штатной работе 0).
    pub fn overwritten(&self) -> u64 {
        self.overwritten
    }
}

impl WhiteBalanceEstimator for CpuWhiteBalanceEstimator {
    fn submit(&mut self, request: EstimationRequest<'_>) -> Result<(), SubmitError> {
        let raw = estimate_scene_light(request.statistics, request.table, &self.config);
        let outcome = finalize_estimate(raw, request.table.band());
        if self.completed.len() >= CPU_COMPLETED_CAPACITY {
            self.completed.pop_front();
            self.overwritten += 1;
        }
        self.completed.push_back(CompletedEstimate {
            frame: request.frame,
            outcome,
        });
        Ok(())
    }

    fn poll(&mut self) -> Option<CompletedEstimate> {
        self.completed.pop_front()
    }
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
