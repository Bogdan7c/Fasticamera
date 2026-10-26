//! Сценарий `motion`: гироскоп на заданной частоте и акселерометр ориентации.
//!
//! Включает оба датчика, ждёт `motion_duration_ms` и считает фактическую частоту гироскопа
//! **по меткам времени датчика** (`CLOCK_BOOTTIME`), а не по моменту доставки — так же, как
//! FPS камеры считается по `SENSOR_TIMESTAMP` (общие правила README). Порог (≥ 380 Гц)
//! проверяет `scripts/smoke_checks.py`.

use std::time::Duration;

use serde::Serialize;
use shell_contract::clock::BoottimeNanos;
use shell_contract::motion::{GyroSample, GyroSubscription, OrientationSubscription};

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, passed};

/// Частота и равномерность потока гироскопа.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GyroRateStats {
    pub samples: usize,
    /// (N − 1) интервалов / (последняя метка − первая).
    pub rate_hz: f64,
    /// Самый длинный интервал между соседними метками.
    pub max_gap_ms: f64,
}

/// Считает частоту по меткам; меньше двух показаний — частота 0.
pub fn gyro_rate_stats(samples: &[GyroSample]) -> GyroRateStats {
    let span = match (samples.first(), samples.last()) {
        (Some(first), Some(last)) if samples.len() >= 2 => last.time.saturating_duration_since(first.time),
        _ => Duration::ZERO,
    };
    let max_gap = samples
        .windows(2)
        .map(|pair| pair[1].time.saturating_duration_since(pair[0].time))
        .max()
        .unwrap_or(Duration::ZERO);
    let rate_hz = if span.is_zero() {
        0.0
    } else {
        (samples.len() - 1) as f64 / span.as_secs_f64()
    };
    GyroRateStats {
        samples: samples.len(),
        rate_hz,
        max_gap_ms: max_gap.as_secs_f64() * 1000.0,
    }
}

#[derive(Debug, Serialize)]
struct GyroReport {
    #[serde(flatten)]
    stats: GyroRateStats,
    requested_rate_hz: u32,
    ring_capacity: usize,
    rejected_out_of_order: u64,
}

#[derive(Debug, Serialize)]
struct MotionReport {
    gyro: GyroReport,
    /// Последняя ориентация из событий оболочки или `unknown`.
    orientation: &'static str,
    accelerometer_samples: u64,
}

/// Работающие датчики и момент старта.
struct RunningSensors {
    gyro: GyroSubscription,
    orientation: OrientationSubscription,
    started_at: std::time::Instant,
}

#[derive(Default)]
pub struct MotionScenario {
    running: Option<RunningSensors>,
}

impl MotionScenario {
    pub fn new() -> Self {
        Self::default()
    }

    fn report(sensors: &RunningSensors, context: &ScenarioContext<'_>) -> MotionReport {
        let ring = sensors.gyro.ring();
        let all_samples =
            ring.samples_between(BoottimeNanos::from_nanos(i64::MIN), BoottimeNanos::from_nanos(i64::MAX));
        MotionReport {
            gyro: GyroReport {
                stats: gyro_rate_stats(&all_samples),
                requested_rate_hz: context.gyro_request.rate.hertz(),
                ring_capacity: ring.capacity().get(),
                rejected_out_of_order: ring.stats().rejected_out_of_order,
            },
            orientation: context
                .display
                .orientation()
                .map_or("unknown", |orientation| orientation.name()),
            accelerometer_samples: sensors.orientation.accelerometer_samples(),
        }
    }
}

impl ObservingScenario for MotionScenario {
    fn name(&self) -> &'static str {
        "motion"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let gyro = match context.services.motion.start_gyro(context.gyro_request) {
            Ok(gyro) => gyro,
            Err(error) => return ScenarioStep::Finished(failed(format!("гироскоп не запущен: {error}"))),
        };
        let orientation = match context.services.motion.start_orientation() {
            Ok(orientation) => orientation,
            Err(error) => return ScenarioStep::Finished(failed(format!("ориентация не запущена: {error}"))),
        };
        self.running = Some(RunningSensors {
            gyro,
            orientation,
            started_at: context.now,
        });
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        // Ориентацию маршрутизатор уже записал в DisplayState; события сценарию не нужны.
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Some(sensors) = &self.running else {
            return ScenarioStep::Continue;
        };
        let duration = Duration::from_millis(context.config.motion_duration_ms);
        if context.now.saturating_duration_since(sensors.started_at) < duration {
            return ScenarioStep::Continue;
        }
        let report = Self::report(sensors, context);
        // Датчики выключаются здесь же: подписки уничтожаются вместе со сценарием.
        self.running = None;
        ScenarioStep::Finished(passed(report))
    }
}

#[cfg(test)]
mod tests {
    use shell_contract::motion::AngularVelocity;

    use super::*;

    fn samples_every(period_ns: i64, count: i64) -> Vec<GyroSample> {
        (0..count)
            .map(|index| GyroSample {
                time: BoottimeNanos::from_nanos(1_000_000_000 + index * period_ns),
                velocity: AngularVelocity {
                    x_rad_per_s: 0.0,
                    y_rad_per_s: 0.0,
                    z_rad_per_s: 0.0,
                },
            })
            .collect()
    }

    #[test]
    fn steady_400_hz_stream_measures_400_hz() {
        let stats = gyro_rate_stats(&samples_every(2_500_000, 1201));
        assert_eq!(stats.samples, 1201);
        assert!((stats.rate_hz - 400.0).abs() < 1e-6, "{}", stats.rate_hz);
        assert!((stats.max_gap_ms - 2.5).abs() < 1e-9);
    }

    #[test]
    fn gap_lowers_rate_and_shows_in_max_gap() {
        let mut samples = samples_every(2_500_000, 400);
        // Дыра 50 мс посередине: убираем 19 показаний.
        samples.drain(200..219);
        let stats = gyro_rate_stats(&samples);
        assert!(stats.rate_hz < 400.0);
        assert!((stats.max_gap_ms - 50.0).abs() < 1e-9, "{}", stats.max_gap_ms);
    }

    #[test]
    fn fewer_than_two_samples_have_zero_rate() {
        assert_eq!(gyro_rate_stats(&[]).rate_hz, 0.0);
        assert_eq!(gyro_rate_stats(&samples_every(1, 1)).rate_hz, 0.0);
    }
}
