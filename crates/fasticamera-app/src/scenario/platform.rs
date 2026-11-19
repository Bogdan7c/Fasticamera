//! Сценарий `platform`: тепло, частота кадров surface и все паттерны хаптики.
//!
//! Ждёт, пока приложение на экране и у него есть surface (частота кадров — свойство surface),
//! затем просит частоту `preferred_frame_rate_hz` и по очереди играет все [`HapticPattern`]
//! с паузой `haptic_interval_ms` — владелец оценивает их на ощупь. В отчёт идёт, каким
//! способом воспроизведён каждый паттерн. После паттернов показывает на экране тестовую подсказку владельцу
//! (`operator_prompt_hold_ms`), снимает её и пишет в отчёт исходы показа и снятия: так на телефоне без
//! участия владельца проверяется, что текст подсказки доходит до экрана (скриншот на стадии
//! `operator_prompt_shown`).

use std::time::{Duration, Instant};

use serde::Serialize;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome};
use shell_contract::haptics::{HapticOutcome, HapticPattern};
use shell_contract::operator_prompt::{OperatorPrompt, OperatorPromptKind};
use shell_contract::thermal::{HeadroomForecast, ThermalReading};

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, passed, stage_fields};

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

/// Итоги проверки подсказки владельцу на экране.
#[derive(Debug, Serialize)]
struct OperatorPromptReport {
    /// Исход показа тестовой подсказки.
    show: CommandOutcome,
    /// Исход снятия подсказки после выдержки.
    clear: CommandOutcome,
    /// Сколько она висела на экране, мс (из конфига).
    hold_ms: u64,
}

#[derive(Debug, Serialize)]
struct PlatformReport {
    thermal: ThermalReport,
    haptics: HapticsReport,
    operator_prompt: OperatorPromptReport,
    frame_rate: FrameRateReport,
}

/// Тестовая подсказка: её читает скрипт (по скриншоту) и человек, если он рядом.
fn probe_prompt() -> OperatorPrompt {
    OperatorPrompt::new(
        OperatorPromptKind::Act,
        "ПРОВЕРКА ПОДСКАЗКИ",
        "сценарий platform: этот текст должен быть виден внизу экрана",
    )
}

/// Подсказка показана и ждёт конца выдержки.
struct ShownProbe {
    shown_at: Instant,
    show: CommandOutcome,
}

/// Что уже сделано, пока идут вибрации.
struct InProgress {
    thermal: ThermalReport,
    frame_rate: FrameRateReport,
    played: Vec<HapticEntry>,
    last_played_at: Option<Instant>,
    /// Тестовая подсказка; `None` — паттерны ещё играют.
    probe: Option<ShownProbe>,
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
            probe: None,
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
        if progress.played.len() < HapticPattern::ALL.len() {
            Self::play_next_pattern(progress, context);
            return ScenarioStep::Continue;
        }
        self.advance_probe(context)
    }

    /// Играет очередной паттерн, если прошла пауза `haptic_interval_ms`.
    fn play_next_pattern(progress: &mut InProgress, context: &ScenarioContext<'_>) {
        let interval = Duration::from_millis(context.config.haptic_interval_ms);
        let due = progress
            .last_played_at
            .is_none_or(|last| context.now.saturating_duration_since(last) >= interval);
        if !due {
            return;
        }
        if let Some(&pattern) = HapticPattern::ALL.get(progress.played.len()) {
            let outcome = context.services.haptics.play(pattern);
            progress.played.push(HapticEntry { pattern, outcome });
            progress.last_played_at = Some(context.now);
        }
    }

    /// Подсказка на экране: показать, выдержать `operator_prompt_hold_ms`, снять и завершить сценарий с отчётом.
    fn advance_probe(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let hold = Duration::from_millis(context.config.operator_prompt_hold_ms);
        let Some(progress) = &mut self.progress else {
            return ScenarioStep::Continue;
        };
        let Some(shown) = &progress.probe else {
            let prompt = probe_prompt();
            let show = context.services.operator_prompt.show(&prompt);
            progress.probe = Some(ShownProbe {
                shown_at: context.now,
                show,
            });
            context.emit_stage(
                "operator_prompt_shown",
                stage_fields([
                    ("prompt", prompt.describe().into()),
                    ("outcome", serde_json::to_value(show).unwrap_or_default()),
                ]),
            );
            return ScenarioStep::Continue;
        };
        if context.now.saturating_duration_since(shown.shown_at) < hold {
            return ScenarioStep::Continue;
        }
        let show = shown.show;
        let clear = context.services.operator_prompt.clear();
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
            operator_prompt: OperatorPromptReport {
                show,
                clear,
                hold_ms: context.config.operator_prompt_hold_ms,
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
