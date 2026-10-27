//! Выборки сценария `raw-stream`, которые копятся по тикам: владение метаданными кадра и
//! задержка «сенсор -> дисплей». Чистая логика без платформы: на вход — снимки рендера, на выход —
//! отчёты, поэтому проверяется синтетическими рядами.
//!
//! * [`MetadataOwnershipTracker`] — «паспорт кадра принадлежит именно этому кадру» (решение 42):
//!   у показанных кадров номер кадра Camera2 растёт строго вместе с меткой сенсора и в темпе съёмки;
//!   если спаривание перепутало результаты, ряд ломается;
//! * [`LatencySampler`] — задержка от начала экспозиции (метка сенсора) до момента, когда кадр
//!   реально появился на экране.

use std::collections::HashMap;

use render_loop::backend::PastPresentation;
use render_loop::camera_diagnostics::{PresentedCameraFrame, PresentedSample};
use serde::Serialize;
use shell_contract::clock::BoottimeNanos;

use crate::clock_offset::ClockOffset;

/// Отчёт проверки владения метаданными.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnershipReport {
    /// Сколько разных показанных кадров попало в выборку.
    pub frames_sampled: u64,
    /// Из них с номером кадра Camera2 (пара-результат пришла).
    pub frames_with_frame_number: u64,
    /// Сколько соседних по выборке пар проверено.
    pub pairs_checked: u64,
    /// Нарушения: метка сенсора или номер кадра не растут, либо шаг номера не соответствует шагу времени.
    pub violations: u64,
    /// Описание первого нарушения (для разбора).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_violation: Option<String>,
    /// Проверка «шаг номера == шаг времени / интервал кадра» пропущена: интервал не измерен.
    pub interval_check_skipped: bool,
    /// Сколько выборок не поместилось в лимит (проверка идёт по помещённым).
    pub samples_over_limit: u64,
}

#[derive(Debug, Clone, Copy)]
struct OwnershipSample {
    sensor_timestamp_nanos: i64,
    frame_number: i64,
}

/// Допуск шага номера кадра относительно шага времени, в кадрах: один кадр может «съехать» из-за
/// округления и джиттера метки.
const FRAME_NUMBER_TOLERANCE: i64 = 1;

/// Копит выборку «метка сенсора — номер кадра» по показанным кадрам.
#[derive(Debug)]
pub struct MetadataOwnershipTracker {
    last_sequence: Option<u64>,
    frames_sampled: u64,
    with_frame_number: Vec<OwnershipSample>,
    limit: usize,
    samples_over_limit: u64,
}

impl MetadataOwnershipTracker {
    /// `limit` — сколько выборок с номером кадра хранить.
    pub fn new(limit: usize) -> Self {
        Self {
            last_sequence: None,
            frames_sampled: 0,
            with_frame_number: Vec::new(),
            limit,
            samples_over_limit: 0,
        }
    }

    /// Учитывает последний показанный кадр, если он новый (по `sequence`). Тик чаще кадра — повтор
    /// того же кадра выборку не раздувает.
    pub fn observe(&mut self, last_presented: Option<PresentedCameraFrame>) {
        let Some(frame) = last_presented else { return };
        if self.last_sequence == Some(frame.sequence) {
            return;
        }
        self.last_sequence = Some(frame.sequence);
        self.frames_sampled += 1;
        let Some(frame_number) = frame.frame_number else { return };
        if self.with_frame_number.len() >= self.limit {
            self.samples_over_limit += 1;
            return;
        }
        self.with_frame_number.push(OwnershipSample {
            sensor_timestamp_nanos: frame.sensor_timestamp_nanos,
            frame_number,
        });
    }

    /// Проверяет выборку. `frame_interval_nanos` — измеренный интервал между кадрами (`None` — не измерен).
    pub fn verdict(&self, frame_interval_nanos: Option<f64>) -> OwnershipReport {
        let mut violations = 0_u64;
        let mut first_violation = None;
        let mut note = |description: String| {
            violations += 1;
            first_violation.get_or_insert(description);
        };
        for pair in self.with_frame_number.windows(2) {
            let (earlier, later) = (pair[0], pair[1]);
            let time_step = later.sensor_timestamp_nanos - earlier.sensor_timestamp_nanos;
            let number_step = later.frame_number - earlier.frame_number;
            if time_step <= 0 {
                note(format!(
                    "метка сенсора не растёт: {} -> {}",
                    earlier.sensor_timestamp_nanos, later.sensor_timestamp_nanos
                ));
                continue;
            }
            if number_step <= 0 {
                note(format!(
                    "номер кадра не растёт: {} -> {} при росте метки на {time_step} нс",
                    earlier.frame_number, later.frame_number
                ));
                continue;
            }
            if let Some(interval) = frame_interval_nanos.filter(|interval| *interval > 0.0) {
                let expected_steps = (time_step as f64 / interval).round() as i64;
                if (number_step - expected_steps).abs() > FRAME_NUMBER_TOLERANCE {
                    note(format!(
                        "шаг номера кадра {number_step} не соответствует шагу времени {time_step} нс (ожидалось около {expected_steps} кадров)"
                    ));
                }
            }
        }
        OwnershipReport {
            frames_sampled: self.frames_sampled,
            frames_with_frame_number: self.with_frame_number.len() as u64 + self.samples_over_limit,
            pairs_checked: self.with_frame_number.len().saturating_sub(1) as u64,
            violations,
            first_violation,
            interval_check_skipped: frame_interval_nanos.is_none_or(|interval| interval <= 0.0),
            samples_over_limit: self.samples_over_limit,
        }
    }
}

/// Отчёт о задержке «начало экспозиции -> кадр на экране».
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LatencyReport {
    /// Сколько кадров замерено.
    pub samples: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p95_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<f64>,
    /// Показы, для которых так и не нашлось времени показа от драйвера (брошены).
    pub unmatched_presentations: u64,
    /// Сопоставленные, но невероятные значения (≤ 0 или выше порога): ошибка сопоставления, не замер.
    pub implausible: u64,
    /// Замеры сверх лимита выборки.
    pub dropped_over_limit: u64,
    /// Сколько раз не удалось прочитать часы (замер за это время потерян).
    pub clock_read_failures: u64,
    /// Описание последней ошибки чтения часов.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_clock_error: Option<String>,
    /// Что именно измерено (для читателя отчёта).
    pub meaning: &'static str,
}

/// Что означает число задержки: метка сенсора — начало экспозиции, а не её конец.
pub const LATENCY_MEANING: &str = "от начала экспозиции (SENSOR_TIMESTAMP) до реального появления кадра на экране";

/// Копит задержки по тикам.
#[derive(Debug)]
pub struct LatencySampler {
    /// Метка сенсора последнего учтённого кадра: более старые уже учтены или брошены.
    high_water_sensor_nanos: Option<i64>,
    samples_nanos: Vec<i64>,
    limit: usize,
    plausible_max_nanos: i64,
    unmatched_presentations: u64,
    implausible: u64,
    dropped_over_limit: u64,
    clock_read_failures: u64,
    last_clock_error: Option<String>,
}

impl LatencySampler {
    pub fn new(limit: usize, plausible_max_ms: u64) -> Self {
        Self {
            high_water_sensor_nanos: None,
            samples_nanos: Vec::new(),
            limit,
            plausible_max_nanos: i64::try_from(plausible_max_ms)
                .unwrap_or(i64::MAX)
                .saturating_mul(1_000_000),
            unmatched_presentations: 0,
            implausible: 0,
            dropped_over_limit: 0,
            clock_read_failures: 0,
            last_clock_error: None,
        }
    }

    /// Чтение часов не удалось: замеры этого тика потеряны, но потеря видна в отчёте.
    pub fn note_clock_failure(&mut self, error: &crate::clock_offset::ClockReadError) {
        self.clock_read_failures += 1;
        self.last_clock_error = Some(error.to_string());
    }

    /// Складывает новые пары «история показов камеры — реальное время показа».
    ///
    /// `history` — старые первыми (метка сенсора растёт). Кадр без найденного времени показа ждёт
    /// следующих тиков (драйвер отдаёт время позже постановки кадра на показ); если за это время
    /// сопоставился более новый кадр, ожидавший считается брошенным.
    pub fn collect(&mut self, history: &[PresentedSample], presentations: &[PastPresentation], offset: ClockOffset) {
        let times: HashMap<u32, PastPresentation> = presentations
            .iter()
            .map(|presentation| (presentation.present_id, *presentation))
            .collect();
        let mut matched = Vec::new();
        let mut pending_unmatched = Vec::new();
        for sample in history {
            if self
                .high_water_sensor_nanos
                .is_some_and(|high_water| sample.sensor_timestamp_nanos <= high_water)
            {
                continue;
            }
            match times.get(&sample.present_id) {
                Some(presentation) => matched.push((*sample, *presentation)),
                None => pending_unmatched.push(sample.sensor_timestamp_nanos),
            }
        }
        let Some(newest_matched) = matched.iter().map(|(sample, _)| sample.sensor_timestamp_nanos).max() else {
            return;
        };
        // Не нашедшие пару и более старые, чем сопоставленный кадр, уже не дождутся пары.
        self.unmatched_presentations += pending_unmatched
            .iter()
            .filter(|sensor_nanos| **sensor_nanos < newest_matched)
            .count() as u64;
        self.high_water_sensor_nanos = Some(newest_matched);
        for (sample, presentation) in matched {
            let on_screen = offset.monotonic_to_boottime(presentation.actual_present);
            let latency = on_screen
                .as_nanos()
                .saturating_sub(BoottimeNanos::from_nanos(sample.sensor_timestamp_nanos).as_nanos());
            self.record(latency);
        }
    }

    fn record(&mut self, latency_nanos: i64) {
        if latency_nanos <= 0 || latency_nanos > self.plausible_max_nanos {
            self.implausible += 1;
        } else if self.samples_nanos.len() >= self.limit {
            self.dropped_over_limit += 1;
        } else {
            self.samples_nanos.push(latency_nanos);
        }
    }

    pub fn report(&self) -> LatencyReport {
        let mut sorted = self.samples_nanos.clone();
        sorted.sort_unstable();
        let to_ms = |nanos: i64| nanos as f64 / 1_000_000.0;
        LatencyReport {
            samples: sorted.len() as u64,
            p50_ms: percentile(&sorted, 50).map(to_ms),
            p95_ms: percentile(&sorted, 95).map(to_ms),
            max_ms: sorted.last().copied().map(to_ms),
            unmatched_presentations: self.unmatched_presentations,
            implausible: self.implausible,
            dropped_over_limit: self.dropped_over_limit,
            clock_read_failures: self.clock_read_failures,
            last_clock_error: self.last_clock_error.clone(),
            meaning: LATENCY_MEANING,
        }
    }
}

/// Перцентиль по ближайшему рангу в отсортированном ряду; пустой ряд — `None`.
fn percentile(sorted: &[i64], percent: usize) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    // Ранг (с единицы) = ceil(percent / 100 * n); индекс — на единицу меньше.
    let rank = (percent * sorted.len()).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

#[cfg(test)]
#[path = "raw_stream_samples_tests.rs"]
mod tests;
