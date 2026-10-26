//! Учёт темпа потока по меткам сенсора (`SENSOR_TIMESTAMP`).
//!
//! Темп считается **только** по меткам: колбэки приходят пачками, а `SENSOR_FRAME_DURATION`
//! в режиме CamX 4:3@60 ложный (33,3 мс при реальных 16,7 мс, 00 C2). Владелец данных —
//! [`CadenceTracker`]: движок скармливает ему результаты текущей сессии и забирает снимок
//! [`StreamStats`].
//!
//! Правила:
//! * интервал считается между соседними по приходу результатами с **растущими** метками;
//! * метка, не большая последней принятой (повтор или откат), считается в
//!   `timestamps_out_of_order` и **не становится «последней»**: единичный запоздавший результат
//!   не порождает ложного огромного интервала на следующем кадре. Цена: если часы меток
//!   навсегда скакнули назад, все следующие результаты тоже уйдут в `timestamps_out_of_order`
//!   — счётчик громко покажет проблему, а ложных интервалов не будет;
//! * пропуск — интервал длиннее `gap_ratio × target`; потерянных кадров в нём
//!   `round(интервал / target) − 1`.

use camera_contract::capabilities::FrameDuration;
use camera_contract::capture_result::{CaptureResultMeta, ResultKey, SensorTimestamp};

use crate::backend::{CaptureFailureReason, SessionGeneration};
use crate::config::CadenceConfig;
use crate::status::{MissingKeyCount, StreamStats};

/// Накопитель статистики потока текущей сессии.
#[derive(Debug, Clone)]
pub struct CadenceTracker {
    config: CadenceConfig,
    stats: StreamStats,
}

impl CadenceTracker {
    pub fn new(config: CadenceConfig) -> Self {
        Self {
            config,
            stats: StreamStats::empty(),
        }
    }

    /// Новая сессия: статистика обнуляется, запоминаются сессия и длительность кадра цели.
    pub fn begin_session(&mut self, session: SessionGeneration, target_frame_duration: FrameDuration) {
        self.stats = StreamStats::empty();
        self.stats.session = Some(session);
        self.stats.target_frame_duration = Some(target_frame_duration);
    }

    /// Снимок статистики.
    pub fn stats(&self) -> &StreamStats {
        &self.stats
    }

    /// Результат запроса целиком (`onCaptureCompleted`).
    pub fn record_completed(&mut self, result: &CaptureResultMeta) {
        self.stats.results_completed += 1;
        if result.reported_frame_duration.is_some() {
            self.stats.last_reported_frame_duration = result.reported_frame_duration;
        }
        self.record_missing_keys(result);
        match result.sensor_timestamp {
            Some(timestamp) => self.record_timestamp(timestamp),
            None => self.stats.results_without_timestamp += 1,
        }
    }

    /// Сбой запроса (`onCaptureFailed`): ошибка HAL или сброс очереди.
    pub fn record_failed(&mut self, reason: CaptureFailureReason) {
        match reason {
            CaptureFailureReason::Error => self.stats.capture_failures += 1,
            CaptureFailureReason::Flushed => self.stats.captures_flushed += 1,
        }
    }

    fn record_missing_keys(&mut self, result: &CaptureResultMeta) {
        for key in result.missing_keys() {
            self.count_missing_key(key);
        }
    }

    fn count_missing_key(&mut self, key: ResultKey) {
        match self.stats.missing_keys.iter_mut().find(|entry| entry.key == key) {
            Some(entry) => entry.results += 1,
            None => self.stats.missing_keys.push(MissingKeyCount { key, results: 1 }),
        }
    }

    fn record_timestamp(&mut self, timestamp: SensorTimestamp) {
        let Some(previous) = self.stats.last_timestamp else {
            self.stats.first_timestamp = Some(timestamp);
            self.stats.last_timestamp = Some(timestamp);
            return;
        };
        // `duration_since` отвергает откат; нулевой интервал (повтор метки) тоже не интервал.
        let interval_nanos = timestamp
            .duration_since(previous)
            .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok())
            .filter(|nanos| *nanos > 0);
        let Some(interval_nanos) = interval_nanos else {
            self.stats.timestamps_out_of_order += 1;
            return;
        };
        self.stats.last_timestamp = Some(timestamp);
        self.record_interval(interval_nanos);
        self.refresh_measured_rate();
    }

    fn record_interval(&mut self, interval_nanos: u64) {
        self.stats.intervals += 1;
        self.stats.min_interval_nanos = Some(
            self.stats
                .min_interval_nanos
                .map_or(interval_nanos, |m| m.min(interval_nanos)),
        );
        self.stats.max_interval_nanos = Some(
            self.stats
                .max_interval_nanos
                .map_or(interval_nanos, |m| m.max(interval_nanos)),
        );
        let Some(target) = self.stats.target_frame_duration.map(FrameDuration::as_nanos) else {
            return;
        };
        if target == 0 {
            return;
        }
        // Числа порядка 1e9..1e11 нс точно представимы в f64.
        if interval_nanos as f64 > self.config.gap_ratio * target as f64 {
            self.stats.gaps += 1;
            let periods = (interval_nanos as f64 / target as f64).round() as u64;
            self.stats.frames_missed_in_gaps += periods.saturating_sub(1);
        }
    }

    /// `intervals / (last − first)` в секундах.
    fn refresh_measured_rate(&mut self) {
        let (Some(first), Some(last)) = (self.stats.first_timestamp, self.stats.last_timestamp) else {
            return;
        };
        let Some(span) = last.duration_since(first) else {
            return;
        };
        let seconds = span.as_secs_f64();
        if seconds > 0.0 {
            self.stats.measured_frames_per_second = Some(self.stats.intervals as f64 / seconds);
        }
    }
}

#[cfg(test)]
#[path = "cadence_tests.rs"]
mod tests;
