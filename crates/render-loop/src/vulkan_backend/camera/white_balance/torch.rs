//! Белая светодиода по кадрам с фонариком предвспышки (сессия 14d).
//!
//! Кадр со вспышкой освещён светодиодом, а не светом сцены, поэтому проявлять его белой видоискателя нельзя.
//! Разовая вспышка и фонарик — один и тот же светодиод, и кадр с фонариком уже пришёл раньше, поэтому белая
//! светодиода берётся из НЕСГЛАЖЕННОЙ оценки такого кадра.
//!
//! Оценки приходят от исполнителя асинхронно и без света кадра, поэтому здесь помнятся метки сенсора кадров
//! с фонариком, чья статистика ушла в оценку: готовая оценка с такой меткой идёт сюда, а НЕ в контроллер AWB
//! видоискателя и не в `last_estimate` (холодный старт берёт лучшую оценку оттуда — свет LED её бы испортил).

use std::collections::VecDeque;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::StreamPassport;
use isp_model::color::chromaticity::Chromaticity;
use std::sync::Arc;

use crate::vulkan_backend::camera::flash_facts::TorchWhite;

/// Сколько кадров с фонариком в оценке помнить (предвспышка — единицы кадров).
const MAX_IN_FLIGHT_FRAMES: usize = 8;

#[derive(Default)]
pub(super) struct TorchWhiteTracker {
    in_flight: VecDeque<i64>,
    ready: Vec<TorchWhite>,
    /// Сколько оценок по кадрам с фонариком получено.
    pub(super) estimated: u64,
    /// Сколько кадров с фонариком оценки не дали (нет серых плиток, ошибка, исполнитель занят).
    pub(super) failed: u64,
}

impl TorchWhiteTracker {
    /// Статистика кадра с фонариком ушла в оценку.
    pub(super) fn note_submitted(&mut self, sensor_timestamp_nanos: i64) {
        if self.in_flight.len() >= MAX_IN_FLIGHT_FRAMES {
            self.in_flight.pop_front();
        }
        self.in_flight.push_back(sensor_timestamp_nanos);
    }

    /// Исполнитель не принял кадр: ответа не будет.
    pub(super) fn withdraw(&mut self, sensor_timestamp_nanos: i64) {
        self.in_flight.retain(|stamp| *stamp != sensor_timestamp_nanos);
        self.failed += 1;
    }

    /// Был ли кадр отправлен как кадр с фонариком (метка забывается: ответ приходит один раз).
    pub(super) fn claim(&mut self, sensor_timestamp_nanos: i64) -> bool {
        let before = self.in_flight.len();
        self.in_flight.retain(|stamp| *stamp != sensor_timestamp_nanos);
        self.in_flight.len() != before
    }

    /// Записывает белую светодиода (оценка кадра `sensor_timestamp_nanos` потока `stream`).
    pub(super) fn record(&mut self, stream: &Arc<StreamPassport>, sensor_timestamp_nanos: i64, white: Chromaticity) {
        self.estimated += 1;
        self.ready.push(TorchWhite {
            stream: Arc::clone(stream),
            frame: SensorTimestamp::from_nanos(sensor_timestamp_nanos),
            white,
        });
    }

    /// Кадр с фонариком оценки не дал.
    pub(super) fn record_failure(&mut self) {
        self.failed += 1;
    }

    /// Забывает всё несданное (новый поток: оценки старого чужие).
    pub(super) fn reset(&mut self) {
        self.in_flight.clear();
        self.ready.clear();
    }

    /// Новые белые светодиода для движка снимков (список становится пустым).
    pub(super) fn drain(&mut self) -> Vec<TorchWhite> {
        std::mem::take(&mut self.ready)
    }
}
