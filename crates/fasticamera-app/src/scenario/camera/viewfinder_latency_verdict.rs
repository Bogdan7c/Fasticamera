//! Сравнение двух профилей `viewfinder-latency`: сводки по профилям (медианы по их окнам) и вердикт
//! «проверяемый профиль лучше / разницы нет / хуже».
//!
//! Вердикт — ДАННЫЕ для решения владельца, а не провал сценария (провалом бывают только поломки замера,
//! см. `evaluate_latency`). По трём критериям ([`Criterion`]) каждая разница между профилями либо
//! значима (не меньше порога из конфига), либо шум:
//! * `processed_fps` — кадров ISP в секунду: больше у проверяемого — лучше;
//! * `latency_p50` — медиана p50 задержки «сенсор → экран»: меньше — лучше;
//! * `miss_ratio` — доля кадров, показанных позже задуманного vsync: меньше — лучше.
//!
//! Итог: есть улучшения и нет ухудшений — `tuned_better`; есть ухудшения и нет улучшений —
//! `tuned_worse`; и то и другое — `mixed` (решает владелец); ничего значимого — `no_clear_difference`;
//! нечего сравнивать — `not_comparable`.

use serde::Serialize;

use super::viewfinder_latency_report::{LatencyWindow, ProfileSlot};
use super::viewfinder_metrics::{median, round2};
use crate::viewfinder_scenario_config::ViewfinderLatencyConfig;

/// Критерий сравнения профилей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Criterion {
    ProcessedFps,
    LatencyP50,
    MissRatio,
}

/// Итог сравнения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    TunedBetter,
    NoClearDifference,
    TunedWorse,
    /// Часть критериев лучше, часть хуже: решает владелец.
    Mixed,
    /// Нет окон или замеров хотя бы одного профиля.
    NotComparable,
}

/// Вердикт с причинами (по каким критериям лучше и по каким хуже).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileVerdict {
    pub verdict: VerdictKind,
    pub better_on: Vec<Criterion>,
    pub worse_on: Vec<Criterion>,
}

/// Итог по одному профилю: медианы по его окнам и суммы счётчиков.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileSummary {
    pub windows: usize,
    /// Кадров ISP в секунду (медиана окон).
    pub processed_fps: Option<f64>,
    /// Кадров сенсора в секунду по счётчику потока RAW (медиана окон).
    pub sensor_fps: Option<f64>,
    /// Кадров, вытесненных более новыми, в секунду (медиана окон).
    pub superseded_per_s: Option<f64>,
    /// Потерь кадров в секунду (вытеснение + переполнение буфера; медиана окон).
    pub dropped_per_s: Option<f64>,
    /// Повторов на экране в секунду (медиана окон).
    pub repeats_per_s: Option<f64>,
    /// Доля кадров, показанных ровно один раз (медиана окон).
    pub shown_once_ratio: Option<f64>,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    /// Доля показанных позже задуманного vsync (медиана окон).
    pub miss_ratio: Option<f64>,
    /// Время кадра на GPU (медианы p50 и p95 окон).
    pub frame_gpu_p50_ms: Option<f64>,
    pub frame_gpu_p95_ms: Option<f64>,
    pub fallbacks_to_preferred: u64,
    pub latch_waits_refused: u64,
    /// Пропущенных тиков и состоявшихся сходов на ближнюю линию (суммы по окнам).
    pub skipped_ticks_for_depth: u64,
    pub depth_reductions: u64,
    pub adpf_reports_sent: u64,
    pub adpf_report_failures: u64,
}

/// Сравнение: сводки, разности и вердикт.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileComparison {
    pub baseline: ProfileSummary,
    pub tuned: ProfileSummary,
    /// Выигрыш частоты ISP проверяемого профиля: tuned − baseline, к/с.
    pub fps_gain: Option<f64>,
    /// Выигрыш задержки: baseline p50 − tuned p50, мс (положительный — проверяемый быстрее).
    pub latency_gain_ms: Option<f64>,
    /// Изменение доли опозданий: tuned − baseline (отрицательное — лучше).
    pub miss_ratio_change: Option<f64>,
    /// Разность p50 «tuned − baseline» по парам, мс (отрицательная — проверяемый быстрее).
    pub pair_p50_deltas_ms: Vec<f64>,
    /// Разброс разностей пар `max − min`, мс.
    pub pair_spread_ms: Option<f64>,
    pub verdict: ProfileVerdict,
}

/// Округление до десятитысячных: для долей (0,0123), где сотые съели бы порог в один процент.
fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// Медиана числа окна по окнам профиля, округлённая до сотых.
fn median_of(windows: &[&LatencyWindow], pick: impl Fn(&LatencyWindow) -> Option<f64>) -> Option<f64> {
    median_rounded(windows, pick, round2)
}

fn median_rounded(
    windows: &[&LatencyWindow],
    pick: impl Fn(&LatencyWindow) -> Option<f64>,
    round: fn(f64) -> f64,
) -> Option<f64> {
    let values: Vec<f64> = windows.iter().filter_map(|window| pick(window)).collect();
    median(&values).map(round)
}

fn summarize(windows: &[LatencyWindow], slot: ProfileSlot) -> ProfileSummary {
    let own: Vec<&LatencyWindow> = windows.iter().filter(|window| window.profile == slot).collect();
    ProfileSummary {
        windows: own.len(),
        processed_fps: median_of(&own, |window| window.processed_fps),
        sensor_fps: median_of(&own, |window| window.sensor_fps),
        superseded_per_s: median_of(&own, |window| window.superseded_per_s),
        dropped_per_s: median_of(&own, |window| window.pacing.dropped_per_s),
        repeats_per_s: median_of(&own, |window| window.pacing.repeats_per_s),
        shown_once_ratio: median_rounded(&own, |window| window.pacing.shown_once_ratio, round4),
        latency_p50_ms: median_of(&own, |window| window.latency.p50_ms),
        latency_p95_ms: median_of(&own, |window| window.latency.p95_ms),
        miss_ratio: median_rounded(&own, LatencyWindow::miss_ratio, round4),
        frame_gpu_p50_ms: median_of(&own, |window| window.frame_gpu_time.map(|time| time.p50_ms)),
        frame_gpu_p95_ms: median_of(&own, |window| window.frame_gpu_time.map(|time| time.p95_ms)),
        fallbacks_to_preferred: own
            .iter()
            .map(|window| window.present_target.fallbacks_to_preferred)
            .sum(),
        latch_waits_refused: own.iter().map(|window| window.present_target.latch_waits_refused).sum(),
        skipped_ticks_for_depth: own
            .iter()
            .map(|window| window.present_target.skipped_ticks_for_depth)
            .sum(),
        depth_reductions: own.iter().map(|window| window.present_target.depth_reductions).sum(),
        adpf_reports_sent: own.iter().map(|window| window.adpf.delta.reports_sent).sum(),
        adpf_report_failures: own.iter().map(|window| window.adpf.delta.report_failures).sum(),
    }
}

/// Разности p50 «tuned − baseline» по парам (пара без одного из окон пропускается).
fn pair_deltas(windows: &[LatencyWindow]) -> Vec<f64> {
    let pairs = windows
        .iter()
        .map(|window| window.pair)
        .max()
        .map_or(0, |last| last + 1);
    (0..pairs)
        .filter_map(|pair| {
            let p50_of = |slot| {
                windows
                    .iter()
                    .find(|window| window.pair == pair && window.profile == slot)
                    .and_then(|window| window.latency.p50_ms)
            };
            Some(round2(p50_of(ProfileSlot::Tuned)? - p50_of(ProfileSlot::Baseline)?))
        })
        .collect()
}

/// Вердикт по трём разностям и порогам конфига. `None` у разности — критерий не сравнивается.
fn decide(
    config: &ViewfinderLatencyConfig,
    fps_gain: Option<f64>,
    latency_gain_ms: Option<f64>,
    miss_ratio_change: Option<f64>,
) -> ProfileVerdict {
    let mut better_on = Vec::new();
    let mut worse_on = Vec::new();
    let mut note = |criterion: Criterion, better: bool, worse: bool| {
        if better {
            better_on.push(criterion);
        }
        if worse {
            worse_on.push(criterion);
        }
    };
    let compared = [fps_gain, latency_gain_ms, miss_ratio_change];
    if let Some(gain) = fps_gain {
        note(
            Criterion::ProcessedFps,
            gain >= config.min_fps_delta,
            gain <= -config.min_fps_delta,
        );
    }
    if let Some(gain) = latency_gain_ms {
        note(
            Criterion::LatencyP50,
            gain >= config.min_latency_gain_ms,
            gain <= -config.min_latency_gain_ms,
        );
    }
    if let Some(change) = miss_ratio_change {
        note(
            Criterion::MissRatio,
            change <= -config.max_miss_ratio_increase,
            change > config.max_miss_ratio_increase,
        );
    }
    let verdict = if compared.iter().all(Option::is_none) {
        VerdictKind::NotComparable
    } else {
        match (better_on.is_empty(), worse_on.is_empty()) {
            (false, true) => VerdictKind::TunedBetter,
            (true, false) => VerdictKind::TunedWorse,
            (false, false) => VerdictKind::Mixed,
            (true, true) => VerdictKind::NoClearDifference,
        }
    };
    ProfileVerdict {
        verdict,
        better_on,
        worse_on,
    }
}

/// Разность `later − earlier`, если есть обе величины.
fn difference(later: Option<f64>, earlier: Option<f64>, round: fn(f64) -> f64) -> Option<f64> {
    Some(round(later? - earlier?))
}

/// Сводка и вердикт по закрытым окнам.
pub fn compare_profiles(config: &ViewfinderLatencyConfig, windows: &[LatencyWindow]) -> ProfileComparison {
    let baseline = summarize(windows, ProfileSlot::Baseline);
    let tuned = summarize(windows, ProfileSlot::Tuned);
    let deltas = pair_deltas(windows);
    let spread = match (
        deltas.iter().copied().reduce(f64::max),
        deltas.iter().copied().reduce(f64::min),
    ) {
        (Some(high), Some(low)) => Some(round2(high - low)),
        _ => None,
    };
    let fps_gain = difference(tuned.processed_fps, baseline.processed_fps, round2);
    let latency_gain_ms = difference(baseline.latency_p50_ms, tuned.latency_p50_ms, round2);
    let miss_ratio_change = difference(tuned.miss_ratio, baseline.miss_ratio, round4);
    ProfileComparison {
        verdict: decide(config, fps_gain, latency_gain_ms, miss_ratio_change),
        baseline,
        tuned,
        fps_gain,
        latency_gain_ms,
        miss_ratio_change,
        pair_p50_deltas_ms: deltas,
        pair_spread_ms: spread,
    }
}

#[cfg(test)]
#[path = "viewfinder_latency_verdict_tests.rs"]
mod tests;
