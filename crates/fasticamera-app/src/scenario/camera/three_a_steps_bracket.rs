//! Шаг g сценария `3a`: серия с брекетингом [2, 4, 8, 16] мс при ISO 400 поверх живого видоискателя.

use std::time::{Duration, Instant};

use camera_contract::control::{ExposureTime, Iso};
use camera_contract::three_a::{BracketExposure, ExposureBracket};
use camera_core::CameraEvent;
use camera_core::three_a_status::{BracketFailure, BracketFrameResult, BracketReport};

use super::engine_watch::RunFailure;
use super::progress::Deadline;
use super::three_a_report::{BracketFrameReport, BracketRunReport, ThreeAObservation};
use super::three_a_steps::{StepEnv, StepProgress, ThreeAStep, invalid_value, sent};
use super::viewfinder_metrics::{WindowBaseline, WindowDelta};

/// Что известно об итоге серии.
struct Outcome {
    captured: bool,
    failure: Option<String>,
    frames: Vec<BracketFrameResult>,
}

/// Шаг g.
#[derive(Default)]
pub struct BracketStep {
    baseline: Option<WindowBaseline>,
    deadline: Option<Deadline>,
    outcome: Option<Outcome>,
    /// До этого момента после итога серии рендер дочитывает её кадры.
    render_settle_until: Option<Instant>,
}

impl BracketStep {
    fn build(env: &StepEnv<'_>) -> Result<ExposureBracket, RunFailure> {
        let config = &env.config.bracket;
        let iso = Iso::new(config.iso).map_err(|error| invalid_value("capture_bracket", error))?;
        let mut frames = Vec::with_capacity(config.exposures_ns.len());
        for exposure_ns in &config.exposures_ns {
            let exposure_time = ExposureTime::new(Duration::from_nanos(*exposure_ns))
                .map_err(|error| invalid_value("capture_bracket", error))?;
            frames.push(BracketExposure { exposure_time, iso });
        }
        ExposureBracket::new(frames, config.frame_limit).map_err(|error| invalid_value("capture_bracket", error))
    }

    fn outcome_of(event: &CameraEvent) -> Option<Outcome> {
        let CameraEvent::BracketCaptured { report } = event else {
            return None;
        };
        Some(match report {
            BracketReport::Captured { frames, .. } => Outcome {
                captured: true,
                failure: None,
                frames: frames.clone(),
            },
            BracketReport::Failed { failure, frames, .. } => Outcome {
                captured: false,
                failure: Some(failure_text(*failure).to_owned()),
                frames: frames.clone(),
            },
        })
    }

    fn report(&self, env: &StepEnv<'_>) -> BracketRunReport {
        let (skipped_delta, dropped_during, presented_during) = match &self.baseline {
            Some(baseline) => {
                let delta = WindowDelta::since(baseline, env.now, env.presentation, &env.raw_counters);
                let pacing = baseline.pacing_since(env.now, env.presentation, &env.raw_counters);
                (
                    delta.isp.bracket_frames_skipped,
                    pacing.dropped,
                    delta.camera_frames_presented,
                )
            }
            None => (0, 0, 0),
        };
        let (captured, failure, frames) = match &self.outcome {
            Some(outcome) => (
                Some(outcome.captured),
                outcome.failure.clone(),
                outcome.frames.as_slice(),
            ),
            None => (None, None, [].as_slice()),
        };
        BracketRunReport {
            captured,
            failure,
            frame_gaps: frame_gaps(frames),
            frames: frames
                .iter()
                .map(|frame| BracketFrameReport {
                    index: frame.index,
                    frame_number: frame.frame_number.0,
                    exposure_ns: frame.exposure_time.map(|time| time.as_nanos() as u64),
                    iso: frame.sensitivity_iso,
                })
                .collect(),
            skipped_delta,
            dropped_during,
            presented_during,
        }
    }
}

fn failure_text(failure: BracketFailure) -> &'static str {
    match failure {
        BracketFailure::SubmitFailed => "бэкенд отверг отправку серии",
        BracketFailure::ResultsTimedOut => "не все кадры серии вернули результат за срок",
        BracketFailure::SessionLost => "сессия закрылась посреди серии",
    }
}

/// Пропуски между номерами соседних кадров серии; откат или повтор номера считается за 1.
pub fn frame_gaps(frames: &[BracketFrameResult]) -> i64 {
    frames
        .windows(2)
        .map(|pair| {
            let step = pair[1].frame_number.0 - pair[0].frame_number.0;
            if step > 0 { step - 1 } else { 1 }
        })
        .sum()
}

impl ThreeAStep for BracketStep {
    fn name(&self) -> &'static str {
        "bracket"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        let bracket = Self::build(env)?;
        self.baseline = Some(WindowBaseline::capture(env.now, env.presentation, env.raw_counters));
        sent("capture_bracket", env.running.handle().capture_bracket(bracket))?;
        self.deadline = Some(Deadline::after_millis(env.now, env.config.bracket.result_timeout_ms));
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        if self.outcome.is_none() {
            self.outcome = env.events.iter().find_map(Self::outcome_of);
            if self.outcome.is_some() {
                self.render_settle_until = Some(env.now + Duration::from_millis(env.config.bracket.render_settle_ms));
                return StepProgress::Pending;
            }
            if self.deadline.is_some_and(|deadline| deadline.is_expired(env.now)) {
                observation.bracket = Some(self.report(env));
                return StepProgress::Done;
            }
            return StepProgress::Pending;
        }
        if self.render_settle_until.is_some_and(|until| env.now < until) {
            return StepProgress::Pending;
        }
        observation.bracket = Some(self.report(env));
        StepProgress::Done
    }
}
