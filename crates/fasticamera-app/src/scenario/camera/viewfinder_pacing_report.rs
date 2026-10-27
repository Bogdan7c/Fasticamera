//! Показатели подачи кадров видоискателя за окно `viewfinder-latency` (решение 53): повторы, потери
//! и доля кадров, показанных ровно один раз. Считается из приращений счётчиков потока RAW и рендера
//! за измеряемую часть окна — чистая функция, без часов и потоков.
//!
//! Определения (единые для обеих политик подачи, чтобы профили сравнивались одной линейкой):
//! * `dropped` — кадры камеры, которые на экран не попали: `dropped_superseded` (вытеснены более
//!   новым — норма `LatestWins`) + `dropped_jitter_overflow` (переполнение буфера — дрейф часов
//!   камеры против экрана, ~1 кадр за оборот фазы). В отчёте обе части видны отдельно.
//! * `repeats` — тики показа, на которых рисовался уже показанный кадр (`camera_frame_redraws`
//!   рендера): видимый зрителю повтор. Считается на стороне рендера, поэтому не зависит от политики
//!   (у `LatestWins` поток пустые опросы не считает). Со стороны потока причины видны отдельно:
//!   `underruns` (буфер заполнен, но пуст) и `priming_polls` (буфер ещё заполняется).
//! * `shown_once_ratio = viewfinder_delivered / frames_published` за окно: доля опубликованных
//!   кадров, выданных рендеру ровно один раз (каждый выданный кадр выдаётся один раз; остальные —
//!   `dropped`, кадры в очереди на границах окна и сброшенные при смене эпохи). Идеал — 1.0;
//!   `None` — в окне не опубликовано ни одного кадра.

use raw_frame_stream::RawStreamCounters;
use serde::Serialize;

use super::viewfinder_metrics::{ratio, round2};

/// Округление долей до десятитысячных: сотые съели бы разницу в доли процента.
fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// Подача кадров видоискателя за измеряемую часть окна.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct PacingWindowReport {
    /// Кадров опубликовано потоком для потребителей за окно.
    pub frames_published: u64,
    /// Кадров выдано рендеру (по одному разу каждый).
    pub delivered: u64,
    /// Потеряно всего: `superseded + jitter_overflow`.
    pub dropped: u64,
    /// Из потерянных: вытеснены более новым кадром.
    pub superseded: u64,
    /// Из потерянных: переполнение джиттер-буфера.
    pub jitter_overflow: u64,
    /// Тиков, на которых рисовался уже показанный кадр (на стороне рендера).
    pub repeats: u64,
    /// Опросов заполненного, но пустого буфера (причина повтора, сторона потока).
    pub underruns: u64,
    /// Опросов буфера, пока он заполнялся (старт, смена эпохи или политики).
    pub priming_polls: u64,
    /// Доля кадров, выданных ровно один раз: `delivered / frames_published`.
    pub shown_once_ratio: Option<f64>,
    /// Потерь в секунду.
    pub dropped_per_s: Option<f64>,
    /// Повторов в секунду.
    pub repeats_per_s: Option<f64>,
}

impl PacingWindowReport {
    /// Из приращения счётчиков потока (`stream_delta`), приращения повторов рендера и длины окна.
    pub fn of_window(stream: &RawStreamCounters, display_repeats: u64, window_ms: u64) -> Self {
        let dropped = stream.dropped_superseded + stream.dropped_jitter_overflow;
        let per_second = |count: u64| ratio(count as f64 * 1_000.0, window_ms as f64).map(round2);
        Self {
            frames_published: stream.frames_published,
            delivered: stream.viewfinder_delivered,
            dropped,
            superseded: stream.dropped_superseded,
            jitter_overflow: stream.dropped_jitter_overflow,
            repeats: display_repeats,
            underruns: stream.viewfinder_underruns,
            priming_polls: stream.viewfinder_polls_while_priming,
            shown_once_ratio: ratio(stream.viewfinder_delivered as f64, stream.frames_published as f64).map(round4),
            dropped_per_s: per_second(dropped),
            repeats_per_s: per_second(display_repeats),
        }
    }
}

#[cfg(test)]
#[path = "viewfinder_pacing_report_tests.rs"]
mod tests;
