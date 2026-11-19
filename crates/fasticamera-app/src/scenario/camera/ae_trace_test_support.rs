//! Общие заготовки тестов `ae-trace`: диагностика AE, результат HAL, синтетический ряд. Только для тестов.

use std::time::Duration;

use camera_contract::auto_exposure::{ExposureAuthority, MeteredArea, ProposalConvergence};
use camera_contract::capture_result::FrameNumber;
use camera_contract::three_a::{RequestOrigin, SceneFlicker, ThreeAProfile};
use camera_core::three_a_status::LastThreeAResult;
use render_loop::viewfinder::{AutoExposureCounters, AutoExposureDiagnostics};

use super::ae_trace_series::{FlickerReading, NewFrame, TraceSample};

/// Диагностика нашего AE с заданными числами; остальное — нейтральное.
pub fn diagnostics(
    observed_ev: Option<f64>,
    target_ev: Option<f64>,
    gain_ev: f32,
    proposals_published: u64,
    directive_generation: Option<u64>,
) -> AutoExposureDiagnostics {
    AutoExposureDiagnostics {
        directive_generation,
        authority: ExposureAuthority::Automatic,
        metered_area: MeteredArea::WholeFrame,
        target_log2_seconds_iso: target_ev,
        observed_scene_log2_seconds_iso: observed_ev,
        display_gain_ev: gain_ev,
        convergence: Some(ProposalConvergence::Settled),
        quality: None,
        last_proposal: None,
        counters: AutoExposureCounters {
            proposals_published,
            ..AutoExposureCounters::default()
        },
    }
}

/// Результат HAL с заданным происхождением, выдержкой и ответом детектора мерцания.
pub fn hal_result(
    frame: i64,
    origin: RequestOrigin,
    exposure_ms: f64,
    iso: u32,
    flicker: Option<SceneFlicker>,
) -> LastThreeAResult {
    LastThreeAResult {
        frame_number: FrameNumber(frame),
        sensor_timestamp: None,
        origin,
        af_state: None,
        ae_state: None,
        focus_distance_diopters: None,
        scene_flicker: flicker,
        exposure_time: Some(Duration::from_secs_f64(exposure_ms / 1_000.0)),
        sensitivity_iso: Some(iso),
    }
}

/// Отсчёт с «ровным» экраном: сенсор, цель и замер совпадают, подтяжки нет, счётчики не растут.
pub fn flat_sample(t_ms: u64, level_ev: f64) -> TraceSample {
    TraceSample {
        t_ms,
        observed_ev: Some(level_ev),
        target_ev: Some(level_ev),
        sensor_ev: Some(level_ev),
        gain_ev: 0.0,
        convergence: Some(ProposalConvergence::Settled),
        proposals_published: 0,
        directive_generation: Some(0),
        profile: ThreeAProfile::Photo,
        new_frame: None,
        latest_flicker: None,
    }
}

/// Новый кадр с выдержкой и ответом детектора.
pub fn new_frame(exposure_ms: Option<f64>, flicker: FlickerReading) -> Option<NewFrame> {
    Some(NewFrame { exposure_ms, flicker })
}

/// Синтетический ряд: шаг `step_ms`, `count` отсчётов; значения даёт `level(t_ms)` для замера, цели и сенсора.
pub fn series(step_ms: u64, count: u64, level: impl Fn(u64) -> f64) -> Vec<TraceSample> {
    (0..count)
        .map(|index| {
            let t_ms = index * step_ms;
            flat_sample(t_ms, level(t_ms))
        })
        .collect()
}
