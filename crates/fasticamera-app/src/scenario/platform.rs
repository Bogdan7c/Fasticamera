//! Сценарий `platform`: тепло, частота кадров surface и все паттерны хаптики.
//!
//! Ждёт, пока приложение на экране и у него есть surface (частота кадров — свойство surface),
//! затем просит частоту `preferred_frame_rate_hz` и по очереди играет все [`HapticPattern`]
//! с паузой `haptic_interval_ms` — владелец оценивает их на ощупь. В отчёт идёт, каким
//! способом воспроизведён каждый паттерн.

use std::time::{Duration, Instant};

use serde::Serialize;
use shell_contract::commands::{FrameRate, FrameRateOutcome};
use shell_contract::haptics::{HapticOutcome, HapticPattern};
use shell_contract::thermal::{HeadroomForecast, ThermalReading};

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, passed};

#[derive(Debug, Serialize)]
struct ThermalReport {
    /// Имя статуса или `unavailable: <причина>`.
    status: String,
    headroom: Option<f32>,
    listener: String,
    policy_pressure: shell_contract::thermal_policy::ThermalPressure,
}

#[derive(Debug, Serialize)]
struct HapticEntry {
    pattern: HapticPattern,
    #[serde(flatten)]
    outcome: HapticOutcome,
}

#[derive(Debug, Serialize)]
struct HapticsReport {
    vibrator: &'static str,
    patterns: Vec<HapticEntry>,
}

#[derive(Debug, Serialize)]
struct FrameRateReport {
    requested_hz: f32,
    #[serde(flatten)]
    outcome: FrameRateOutcome,
}

#[derive(Debug, Serialize)]
struct PlatformReport {
    thermal: ThermalReport,
    haptics: HapticsReport,
    frame_rate: FrameRateReport,
}

/// Что уже сделано, пока идут вибрации.
struct InProgress {
    thermal: ThermalReport,
    frame_rate: FrameRateReport,
    played: Vec<HapticEntry>,
    last_played_at: Option<Instant>,
}

#[derive(Default)]
pub struct PlatformScenario {
    progress: Option<InProgress>,
}

impl PlatformScenario {
    pub fn new() -> Self {
        Self::default()
    }

    /// Первый шаг на экране: тепло и частота кадров.
    fn begin(context: &ScenarioContext<'_>) -> Result<InProgress, String> {
        let rate = FrameRate::new(context.config.preferred_frame_rate_hz).map_err(|error| error.to_string())?;
        Ok(InProgress {
            thermal: thermal_report(context),
            frame_rate: FrameRateReport {
                requested_hz: rate.hertz(),
                outcome: context.services.commands.set_preferred_frame_rate(rate),
            },
            played: Vec::with_capacity(HapticPattern::ALL.len()),
            last_played_at: None,
        })
    }

    fn advance(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if self.progress.is_none() {
            if !context.display.is_presenting() {
                return ScenarioStep::Continue;
            }
            match Self::begin(context) {
                Ok(progress) => self.progress = Some(progress),
                Err(error) => return ScenarioStep::Finished(failed(error)),
            }
        }
        let Some(progress) = &mut self.progress else {
            return ScenarioStep::Continue;
        };
        let interval = Duration::from_millis(context.config.haptic_interval_ms);
        let due = progress
            .last_played_at
            .is_none_or(|last| context.now.saturating_duration_since(last) >= interval);
        if !due {
            return ScenarioStep::Continue;
        }
        if let Some(&pattern) = HapticPattern::ALL.get(progress.played.len()) {
            let outcome = context.services.haptics.play(pattern);
            progress.played.push(HapticEntry { pattern, outcome });
            progress.last_played_at = Some(context.now);
            return ScenarioStep::Continue;
        }
        let Some(done) = self.progress.take() else {
            return ScenarioStep::Continue;
        };
        ScenarioStep::Finished(passed(PlatformReport {
            thermal: done.thermal,
            haptics: HapticsReport {
                vibrator: if context.services.facts.has_vibrator() {
                    "present"
                } else {
                    "absent"
                },
                patterns: done.played,
            },
            frame_rate: done.frame_rate,
        }))
    }
}

/// Показание тепла: последнее из событий монитора (не дёргаем источник лишний раз — Android
/// ограничивает частоту запроса запаса), а если его ещё не было — прямой запрос.
fn thermal_report(context: &ScenarioContext<'_>) -> ThermalReport {
    let reading: Result<ThermalReading, String> = match context.thermal_policy.latest_reading() {
        Some(reading) => Ok(reading),
        None => context
            .services
            .thermal
            .current_status()
            .map(|status| ThermalReading {
                status,
                headroom: context.services.thermal.headroom(HeadroomForecast::NOW),
            })
            .map_err(|error| error.to_string()),
    };
    let (status, headroom) = match reading {
        Ok(reading) => (reading.status.name().to_owned(), reading.headroom.measured()),
        Err(error) => (format!("unavailable: {error}"), None),
    };
    ThermalReport {
        status,
        headroom,
        listener: context.services.facts.thermal_listener().report_text(),
        policy_pressure: context.thermal_policy.pressure(),
    }
}

impl ObservingScenario for PlatformScenario {
    fn name(&self) -> &'static str {
        "platform"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }

    fn on_event(&mut self, _event: ObservedEvent, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }
}
