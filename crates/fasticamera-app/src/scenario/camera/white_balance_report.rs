//! Отчёт о нашем AWB для сценариев `viewfinder` и `3a` (сессия 08): одно место, где диагностика
//! потока рендера ([`WhiteBalanceDiagnostics`], без `Serialize`) превращается в компактный JSON.
//!
//! Белая HAL здесь только для сравнения (`hal_delta_*`): в ISP она не попадает (решение 62).
//! Числа округлены, а счётчики из `scene-estimation` (`decisions`) в отчёт не тянем — сценариям
//! они не нужны, а строка результата ограничена 4000 байтами.

use render_loop::viewfinder::{ColdStartState, WhiteBalanceDiagnostics};
use serde::Serialize;

use super::viewfinder_metrics::round2;

/// Белая точка: температура и Duv.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhitePointReport {
    pub kelvin: f64,
    pub duv: f64,
}

/// Состояние AWB на момент снимка.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhiteBalanceReport {
    /// Оценок света получено за сессию показа.
    pub estimates: u64,
    /// Белая, которой считается ISP (`None` — белой сцены ещё нет).
    pub our_white: Option<WhitePointReport>,
    /// Уверенность последней оценки `[0, 1]`.
    pub confidence: Option<f64>,
    /// Принятые серые плитки последней оценки.
    pub accepted_tiles: Option<u32>,
    /// Наша белая минус белая HAL, майреды (плюс — наша теплее). Только для сравнения.
    pub hal_delta_mired: Option<f64>,
    /// Наша белая минус белая HAL по Duv.
    pub hal_delta_duv: Option<f64>,
    /// Холодный старт идёт сейчас (кадры скрываются).
    pub cold_start_pending: bool,
    /// Скрыто кадров на холодных стартах за сессию показа.
    pub hidden_frames: u64,
    /// Сколько раз скрытие кончилось принудительным показом (лимит исчерпан без оценки).
    pub cold_start_forced: u64,
    /// Каждый N-й кадр считает статистику (конфиг рендера).
    pub statistics_every_n_frames: u32,
    pub estimation_failures: u64,
    /// Таблицу локуса построить не удалось: AWB не работает.
    pub table_failure: Option<String>,
}

impl WhiteBalanceReport {
    pub fn of(diagnostics: &WhiteBalanceDiagnostics) -> Self {
        Self {
            estimates: diagnostics.estimates,
            our_white: diagnostics.scene_white.map(|white| WhitePointReport {
                kelvin: white.temperature_kelvin.round(),
                duv: round4(white.duv),
            }),
            confidence: diagnostics
                .last_estimate
                .map(|estimate| round2(f64::from(estimate.confidence))),
            accepted_tiles: diagnostics.last_estimate.map(|estimate| estimate.accepted_tiles),
            hal_delta_mired: diagnostics.hal_delta.map(|delta| round2(delta.mired)),
            hal_delta_duv: diagnostics.hal_delta.map(|delta| round4(delta.duv)),
            cold_start_pending: matches!(diagnostics.cold_start, ColdStartState::Pending { .. }),
            hidden_frames: diagnostics.hidden_frames_cold_start,
            cold_start_forced: diagnostics.cold_start_forced,
            statistics_every_n_frames: diagnostics.statistics_every_n_frames,
            estimation_failures: diagnostics.estimation_failures,
            table_failure: diagnostics.table_failure.clone(),
        }
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
#[path = "white_balance_report_tests.rs"]
mod tests;
