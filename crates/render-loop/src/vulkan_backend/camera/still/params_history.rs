//! История параметров обработанных кадров: белая точка и подтяжка `k`, с которыми видоискатель показал кадр.
//!
//! Снимок обязан повторить параметры СВОЕГО кадра (решение 72; белая — решение нашего AWB в тот момент, а не
//! сегодняшняя). Кольцо снимков хранит и кадры, которые видоискатель не обрабатывал (замена выбора, кадр между
//! двумя показами), — для них берётся запись ближайшего по времени кадра ТОГО ЖЕ потока, и это считается
//! отдельно. Чужой поток (`Arc::ptr_eq` не совпал) не подходит никогда: его уровни и матрицы другие (решение 42).

use std::collections::VecDeque;
use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::StreamPassport;

use super::feed::FrameStillParams;

/// Что нашлось для кадра снимка.
#[derive(Debug, Clone)]
pub enum ParamsLookup {
    /// Запись самого кадра.
    Own(FrameStillParams),
    /// Своей записи нет: запись ближайшего кадра того же потока.
    NearestOfSameStream {
        params: FrameStillParams,
        /// Расстояние между метками (шкала сенсора, `BOOTTIME`: только разность двух меток одной шкалы).
        distance_nanos: u64,
    },
    /// Записей этого потока нет вовсе.
    Missing,
}

/// Кольцо записей параметров по метке сенсора.
#[derive(Debug)]
pub struct FrameParamsHistory {
    entries: VecDeque<(SensorTimestamp, FrameStillParams)>,
    capacity: usize,
}

impl FrameParamsHistory {
    /// История на `capacity` кадров.
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Запоминает параметры обработанного кадра; старейшая запись вытесняется по ёмкости.
    pub fn record(&mut self, sensor_timestamp: SensorTimestamp, params: FrameStillParams) {
        self.entries.push_back((sensor_timestamp, params));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// Забыть всё (surface отпущена: поток закончился).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Сколько записей помнит история (тесты).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Пуста ли история (тесты).
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Параметры для кадра `sensor_timestamp` потока `stream` (см. модуль).
    pub fn lookup(&self, sensor_timestamp: SensorTimestamp, stream: &Arc<StreamPassport>) -> ParamsLookup {
        let same_stream = || {
            self.entries
                .iter()
                .filter(|(_, params)| Arc::ptr_eq(&params.stream, stream))
        };
        if let Some((_, own)) = same_stream().find(|(stamp, _)| *stamp == sensor_timestamp) {
            return ParamsLookup::Own(own.clone());
        }
        // Ближайший по расстоянию; при равенстве — более ранний (он «не позже, чем видел пользователь»).
        let nearest =
            same_stream().min_by_key(|(stamp, _)| (stamp.as_nanos().abs_diff(sensor_timestamp.as_nanos()), *stamp));
        match nearest {
            Some((stamp, params)) => ParamsLookup::NearestOfSameStream {
                params: params.clone(),
                distance_nanos: stamp.as_nanos().abs_diff(sensor_timestamp.as_nanos()),
            },
            None => ParamsLookup::Missing,
        }
    }
}

#[cfg(test)]
#[path = "params_history_tests.rs"]
mod tests;
