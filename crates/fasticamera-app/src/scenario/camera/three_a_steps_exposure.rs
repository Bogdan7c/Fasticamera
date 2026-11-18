//! Шаги `3a` про экспозицию: d (ручная выдержка и ISO, слишком длинная выдержка) и e (компенсация EV).

use std::time::Duration;

use camera_contract::capabilities::ManualSensorRanges;
use camera_contract::control::{Control, ExposureBias, ExposureTime, Iso, ManualControl};
use camera_contract::three_a::AeState;
use camera_core::CameraEvent;
use camera_core::three_a_status::{AdjustmentReason, ControlParameter};

use super::engine_watch::RunFailure;
use super::progress::Deadline;
use super::three_a_own_ae::{AeSource, is_converged, is_limit_reached};
use super::three_a_report::{
    AppliedExposure, EvPointReport, EvReport, LongShutterReport, ManualReport, ThreeAObservation,
};
use super::three_a_results::{exposure_product, median, relative_gap};
use super::three_a_steps::{StepEnv, StepProgress, ThreeAStep, invalid_value, sent};

/// Ручные значения: ISO и выдержка закреплены, фокус и баланс на автомате.
fn manual_exposure(iso: u32, shutter_ns: u64) -> Result<ManualControl, RunFailure> {
    let iso = Iso::new(iso).map_err(|error| invalid_value("apply_controls", error))?;
    let shutter =
        ExposureTime::new(Duration::from_nanos(shutter_ns)).map_err(|error| invalid_value("apply_controls", error))?;
    Ok(ManualControl {
        iso: Control::Manual(iso),
        shutter: Control::Manual(shutter),
        ..ManualControl::AUTOMATIC
    })
}

/// Фазы шага d.
enum ManualPhase {
    WaitApplied {
        frame_at_send: Option<i64>,
        deadline: Deadline,
    },
    WaitLimited {
        deadline: Deadline,
        marker: Option<i64>,
    },
}

/// Шаг d: ISO 400 + 1/1000 с → в результате ровно они; затем 1/10 с → ограничение длительностью кадра.
#[derive(Default)]
pub struct ManualStep {
    phase: Option<ManualPhase>,
    applied: Option<AppliedExposure>,
    adjustments_in_range: usize,
    limited_event_seen: bool,
}

impl ManualStep {
    fn limited_by_frame_duration(event: &CameraEvent) -> bool {
        matches!(event, CameraEvent::ControlsAdjusted { adjustments }
            if adjustments.iter().any(|adjustment| adjustment.parameter == ControlParameter::Shutter
                && adjustment.reason == AdjustmentReason::LimitedByFrameDuration))
    }

    fn poll_applied(&mut self, env: &mut StepEnv<'_>, frame_at_send: Option<i64>, deadline: Deadline) -> StepProgress {
        let tolerance = env.config.manual.tolerance_fraction;
        let (shutter, iso) = (env.config.manual.shutter_ns, env.config.manual.iso);
        // Все поправки с момента команды — на значения внутри диапазона их быть не должно.
        self.adjustments_in_range += env
            .events
            .iter()
            .filter(|event| matches!(event, CameraEvent::ControlsAdjusted { .. }))
            .count();
        let matching = env.results.repeating_after(frame_at_send).find(|result| {
            let exposure_ok = result
                .exposure_time
                .and_then(|time| relative_gap(time.as_nanos() as f64, shutter as f64))
                .is_some_and(|gap| gap <= tolerance);
            let iso_ok = result
                .sensitivity_iso
                .and_then(|value| relative_gap(f64::from(value), f64::from(iso)))
                .is_some_and(|gap| gap <= tolerance);
            exposure_ok && iso_ok
        });
        if let Some(result) = matching {
            self.applied = Some(AppliedExposure {
                exposure_ns: result.exposure_time.map_or(0, |time| time.as_nanos() as u64),
                iso: result.sensitivity_iso.unwrap_or(0),
                latency_frames: result.frame_number.0 - frame_at_send.unwrap_or(result.frame_number.0),
            });
        } else if !deadline.is_expired(env.now) {
            self.phase = Some(ManualPhase::WaitApplied {
                frame_at_send,
                deadline,
            });
            return StepProgress::Pending;
        }
        self.send_long_shutter(env)
    }

    fn send_long_shutter(&mut self, env: &mut StepEnv<'_>) -> StepProgress {
        let command = manual_exposure(env.config.manual.iso, env.config.manual.long_shutter_ns)
            .and_then(|controls| sent("apply_controls", env.running.handle().apply_controls(controls)));
        match command {
            Ok(_) => {
                self.phase = Some(ManualPhase::WaitLimited {
                    deadline: Deadline::after_millis(env.now, env.config.manual.apply_timeout_ms),
                    marker: env.results.latest_frame(),
                });
                StepProgress::Pending
            }
            Err(failure) => StepProgress::Failed(failure),
        }
    }

    fn poll_limited(
        &mut self,
        env: &mut StepEnv<'_>,
        observation: &mut ThreeAObservation,
        marker: Option<i64>,
        deadline: Deadline,
    ) -> StepProgress {
        self.limited_event_seen |= env.events.iter().any(Self::limited_by_frame_duration);
        let budget_ns = env
            .status
            .stream
            .target_frame_duration
            .map(|duration| duration.as_nanos());
        // Результат с уже ограниченной выдержкой: заметно длиннее ручной в диапазоне (устаревшие кадры
        // до применения команды короче).
        let threshold = env.config.manual.shutter_ns.saturating_mul(2);
        let limited_result = env.results.repeating_after(marker).find(|result| {
            result
                .exposure_time
                .is_some_and(|time| time.as_nanos() as u64 >= threshold)
        });
        let done = (limited_result.is_some() && self.limited_event_seen) || deadline.is_expired(env.now);
        if !done {
            self.phase = Some(ManualPhase::WaitLimited { deadline, marker });
            return StepProgress::Pending;
        }
        observation.manual = Some(ManualReport {
            requested_exposure_ns: env.config.manual.shutter_ns,
            requested_iso: env.config.manual.iso,
            applied: self.applied,
            adjustments_for_in_range: self.adjustments_in_range,
            long_shutter: LongShutterReport {
                requested_ns: env.config.manual.long_shutter_ns,
                frame_budget_ns: budget_ns,
                limited_event_seen: self.limited_event_seen,
                applied_ns: limited_result
                    .and_then(|result| result.exposure_time)
                    .map(|time| time.as_nanos() as u64),
            },
        });
        StepProgress::Done
    }
}

impl ThreeAStep for ManualStep {
    fn name(&self) -> &'static str {
        "manual"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        let controls = manual_exposure(env.config.manual.iso, env.config.manual.shutter_ns)?;
        sent("apply_controls", env.running.handle().apply_controls(controls))?;
        self.phase = Some(ManualPhase::WaitApplied {
            frame_at_send: env.results.latest_frame(),
            deadline: Deadline::after_millis(env.now, env.config.manual.apply_timeout_ms),
        });
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        match self.phase.take() {
            Some(ManualPhase::WaitApplied {
                frame_at_send,
                deadline,
            }) => self.poll_applied(env, frame_at_send, deadline),
            Some(ManualPhase::WaitLimited { deadline, marker }) => {
                self.poll_limited(env, observation, marker, deadline)
            }
            None => StepProgress::Done,
        }
    }
}

/// Пределы «выдержка × ISO» камеры в нс·ISO: (наименьший, наибольший). `None` — ручной сенсор не поддержан.
pub fn product_limits(env: &StepEnv<'_>) -> Option<(f64, f64)> {
    let stream = &env.status.configured.as_ref()?.stream;
    let camera = env.status.catalog.camera(&stream.camera)?;
    let ManualSensorRanges::Supported { iso, exposure_time, .. } = camera.capabilities.manual_sensor else {
        return None;
    };
    let longest = env
        .status
        .stream
        .target_frame_duration
        .map_or(exposure_time.longest, |frame| {
            frame.as_duration().min(exposure_time.longest)
        });
    Some((
        exposure_time.shortest.as_nanos() as f64 * f64::from(iso.min),
        longest.as_nanos() as f64 * f64::from(iso.max),
    ))
}

/// Запас от пределов сенсора, внутри которого EV считаем проверяемым: у самого предела AE упирается в него.
const LIMIT_MARGIN: f64 = 0.05;

/// Укладывается ли ожидаемая энергия кадра в пределы сенсора с запасом: у самого предела AE упирается в него
/// и компенсацию проверить нельзя. Пределов нет (ручной сенсор не поддержан) — проверить нечем.
pub fn is_verifiable_energy(expected: f64, limits: Option<(f64, f64)>) -> bool {
    limits.is_some_and(|(lowest, highest)| {
        expected >= lowest * (1.0 + LIMIT_MARGIN) && expected <= highest * (1.0 - LIMIT_MARGIN)
    })
}

/// Фаза измерения одной компенсации.
enum EvPhase {
    /// Ждём `until`; наш AE после него ждёт ещё и `Settled`/`Holding`/`LimitReached`, но не дольше `hard_deadline`.
    Settling {
        until: std::time::Instant,
        hard_deadline: Deadline,
    },
    Collecting {
        marker: Option<i64>,
        deadline: Deadline,
    },
}

/// Шаг e: «выдержка × ISO» при компенсации 0, +1, −1 EV (вендорский AE квантует её шагом 1/6 EV).
#[derive(Default)]
pub struct EvStep {
    index: usize,
    phase: Option<EvPhase>,
    points: Vec<EvPointReport>,
}

impl EvStep {
    fn apply_bias(&self, env: &StepEnv<'_>, bias_ev: f32) -> Result<(), RunFailure> {
        let bias = ExposureBias::from_ev(bias_ev).map_err(|error| invalid_value("set_exposure_bias", error))?;
        sent("set_exposure_bias", env.running.handle().set_exposure_bias(bias)).map(|_| ())
    }

    fn start_settling(&mut self, env: &StepEnv<'_>) {
        self.phase = Some(EvPhase::Settling {
            until: env.now + Duration::from_millis(env.config.ev.settle_ms),
            hard_deadline: Deadline::after_millis(env.now, env.config.ev.settle_ms + env.config.ev.collect_timeout_ms),
        });
    }

    fn record_point(&mut self, env: &StepEnv<'_>, marker: Option<i64>) {
        let samples: Vec<_> = env
            .results
            .repeating_after(marker)
            .take(env.config.ev.samples)
            .collect();
        let products: Vec<f64> = samples.iter().filter_map(|result| exposure_product(result)).collect();
        self.points.push(EvPointReport {
            bias_ev: env.config.ev.biases_ev[self.index],
            median_product: median(products),
            samples: samples.len(),
            ae_converged: match AeSource::of(&env.status.three_a) {
                AeSource::Vendor => samples
                    .last()
                    .is_some_and(|result| result.ae_state == Some(AeState::Converged)),
                AeSource::Own => is_converged(env.status.three_a.auto_exposure),
            },
            ae_limit_reached: is_limit_reached(env.status.three_a.auto_exposure),
            verifiable: true,
            ratio_to_baseline: None,
        });
    }

    /// Отношения к опорной точке и проверяемость по пределам сенсора.
    fn finish(&mut self, env: &StepEnv<'_>) -> EvReport {
        let baseline = self.points.first().and_then(|point| point.median_product);
        let limits = product_limits(env);
        for point in &mut self.points {
            point.ratio_to_baseline = point.median_product.zip(baseline).map(|(product, base)| product / base);
            if point.bias_ev != 0.0 {
                let expected = baseline.map(|base| base * 2.0_f64.powf(f64::from(point.bias_ev)));
                point.verifiable = expected.is_some_and(|expected| is_verifiable_energy(expected, limits));
            }
            // Наш AE сам сказал, что упёрся в предел: произведение там не равно цели, проверять нечего.
            if point.ae_limit_reached {
                point.verifiable = false;
            }
        }
        EvReport {
            ae_source: AeSource::of(&env.status.three_a),
            ev_unverifiable_at_limit: self.points.iter().any(|point| !point.verifiable),
            points: std::mem::take(&mut self.points),
        }
    }
}

impl ThreeAStep for EvStep {
    fn name(&self) -> &'static str {
        "ev"
    }

    fn begin(&mut self, env: &mut StepEnv<'_>, _observation: &mut ThreeAObservation) -> Result<(), RunFailure> {
        // Назад на автомат: компенсация 0 — опорная точка.
        sent(
            "apply_controls",
            env.running.handle().apply_controls(ManualControl::AUTOMATIC),
        )?;
        self.start_settling(env);
        Ok(())
    }

    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress {
        match self.phase.take() {
            Some(EvPhase::Settling { until, hard_deadline }) => {
                let progress = env.status.three_a.auto_exposure;
                let own_still_moving = AeSource::of(&env.status.three_a) == AeSource::Own
                    && !(is_converged(progress) || is_limit_reached(progress))
                    && !hard_deadline.is_expired(env.now);
                if env.now < until || own_still_moving {
                    self.phase = Some(EvPhase::Settling { until, hard_deadline });
                } else {
                    self.phase = Some(EvPhase::Collecting {
                        marker: env.results.latest_frame(),
                        deadline: Deadline::after_millis(env.now, env.config.ev.collect_timeout_ms),
                    });
                }
                StepProgress::Pending
            }
            Some(EvPhase::Collecting { marker, deadline }) => {
                let collected = env.results.repeating_after(marker).count();
                if collected < env.config.ev.samples && !deadline.is_expired(env.now) {
                    self.phase = Some(EvPhase::Collecting { marker, deadline });
                    return StepProgress::Pending;
                }
                self.record_point(env, marker);
                self.index += 1;
                match env.config.ev.biases_ev.get(self.index).copied() {
                    Some(bias_ev) => match self.apply_bias(env, bias_ev) {
                        Ok(()) => {
                            self.start_settling(env);
                            StepProgress::Pending
                        }
                        Err(failure) => StepProgress::Failed(failure),
                    },
                    None => {
                        observation.ev = Some(self.finish(env));
                        StepProgress::Done
                    }
                }
            }
            None => StepProgress::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Option<(f64, f64)> = Some((1.0e6, 1.0e9));

    #[test]
    fn energy_inside_the_limits_with_margin_is_verifiable() {
        assert!(is_verifiable_energy(1.0e8, LIMITS));
    }

    #[test]
    fn energy_at_or_beyond_the_upper_limit_is_not_verifiable() {
        assert!(!is_verifiable_energy(0.99e9, LIMITS), "внутри запаса 5 % от потолка");
        assert!(!is_verifiable_energy(2.0e9, LIMITS));
    }

    #[test]
    fn energy_at_or_below_the_lower_limit_is_not_verifiable() {
        assert!(!is_verifiable_energy(1.02e6, LIMITS), "внутри запаса 5 % от пола");
        assert!(!is_verifiable_energy(1.0e5, LIMITS));
    }

    #[test]
    fn without_sensor_limits_nothing_is_verifiable() {
        assert!(!is_verifiable_energy(1.0e8, None));
    }
}
