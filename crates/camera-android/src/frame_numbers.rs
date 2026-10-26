//! Номер кадра для результата запроса.
//!
//! Колбэк результата NDK (`onCaptureCompleted`) номер кадра **не отдаёт**, а колбэк начала
//! кадра V2 (`onCaptureStarted`, API 33) — отдаёт вместе с меткой сенсора. Поэтому на старте
//! кадра запоминаем пару «метка сенсора → номер кадра», а на результате достаём номер по метке
//! `SENSOR_TIMESTAMP`. Пара не нашлась (старт потерян или окно переполнилось) — честный запасной
//! счётчик, и это считается: подставной номер никогда не выдаётся за настоящий молча.
//!
//! Запасные номера **отрицательные** (−1, −2, …): настоящие номера кадров NDK неотрицательны
//! (`ACaptureRequest` нумерует с 0), поэтому запасной номер не может совпасть с настоящим, даже
//! если оба встретятся в одном потоке результатов.

use std::collections::VecDeque;

use camera_contract::capture_result::{FrameNumber, SensorTimestamp};

/// Откуда взят номер кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameNumberSource {
    /// Настоящий номер из `onCaptureStarted`.
    Reported(FrameNumber),
    /// Пары не нашлось; номер — запасной счётчик (убывающий от −1, всегда отрицательный: не совпадает с настоящим).
    Fallback(FrameNumber),
}

impl FrameNumberSource {
    pub fn frame_number(self) -> FrameNumber {
        match self {
            FrameNumberSource::Reported(number) | FrameNumberSource::Fallback(number) => number,
        }
    }
}

/// Скользящее окно пар «метка → номер» фиксированной ёмкости.
#[derive(Debug)]
pub struct FrameNumberMatcher {
    capacity: usize,
    started: VecDeque<(SensorTimestamp, FrameNumber)>,
    next_fallback: i64,
}

impl FrameNumberMatcher {
    /// `capacity` — число кадров в полёте, которое матчер переживает (минимум 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            started: VecDeque::new(),
            next_fallback: -1,
        }
    }

    /// Запомнить начало кадра. При переполнении вытесняется самая старая пара.
    pub fn record_started(&mut self, timestamp: SensorTimestamp, frame_number: FrameNumber) {
        if self.started.len() == self.capacity {
            self.started.pop_front();
        }
        self.started.push_back((timestamp, frame_number));
    }

    /// Номер кадра для результата с этой меткой (`None` — у результата метки нет).
    ///
    /// Найденная пара удаляется: результат одного кадра приходит один раз.
    pub fn resolve(&mut self, timestamp: Option<SensorTimestamp>) -> FrameNumberSource {
        let position = timestamp.and_then(|wanted| self.started.iter().position(|(stamp, _)| *stamp == wanted));
        match position.and_then(|index| self.started.remove(index)) {
            Some((_, frame_number)) => FrameNumberSource::Reported(frame_number),
            None => {
                let number = FrameNumber(self.next_fallback);
                self.next_fallback = self.next_fallback.saturating_sub(1);
                FrameNumberSource::Fallback(number)
            }
        }
    }
}

#[cfg(test)]
#[path = "frame_numbers_tests.rs"]
mod tests;
