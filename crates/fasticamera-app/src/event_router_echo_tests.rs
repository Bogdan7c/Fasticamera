//! Повтор строки результата (решение 58): маршрутизатор выдаёт ту же строку итога ещё N раз
//! разнесённо во времени и отменяет повторы, когда пришёл новый сценарий.

use super::*;

/// Сценарий, который успешно заканчивается на первом тике.
struct FinishOnTickScenario;

impl ObservingScenario for FinishOnTickScenario {
    fn name(&self) -> &'static str {
        "finish-on-tick"
    }

    fn start(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Finished(ScenarioOutcome::Passed {
            report: serde_json::json!({"marker": 7}),
        })
    }
}

fn echo_config(repeats: u32, interval_ms: u64) -> ScenarioConfig {
    ScenarioConfig {
        result_echo: crate::result_echo::ResultEchoConfig { repeats, interval_ms },
        ..ScenarioConfig::default()
    }
}

impl Harness {
    /// Крутит маршрутизатор без событий заданное время (тики по 2 мс).
    fn idle_for(&mut self, duration: Duration) {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.router.step(Duration::from_millis(2));
        }
    }

    fn repeats(&self) -> Vec<(Instant, serde_json::Value)> {
        self.output.repeats.lock().expect("замок").clone()
    }
}

#[test]
fn result_is_repeated_exactly_the_configured_number_of_times_spaced_by_the_interval() {
    let mut harness = harness(echo_config(3, 20));
    harness.starter.start(Box::new(FinishOnTickScenario)).expect("старт");
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "passed");

    harness.idle_for(Duration::from_millis(300));

    // Итог один, повторов ровно 3, каждый — тот же итог, и между копиями не меньше интервала.
    let repeats = harness.repeats();
    assert_eq!(harness.output.single_result(), result);
    assert_eq!(repeats.len(), 3, "повторы: {repeats:?}");
    assert!(repeats.iter().all(|(_, repeated)| *repeated == result));
    for pair in repeats.windows(2) {
        assert!(
            pair[1].0 - pair[0].0 >= Duration::from_millis(20),
            "копии пачкой: {repeats:?}"
        );
    }
}

#[test]
fn a_new_scenario_cancels_the_repeats_of_the_previous_result() {
    let mut harness = harness(echo_config(5, 40));
    harness.starter.start(Box::new(FinishOnTickScenario)).expect("старт");
    harness.run_until_result();
    // Новый сценарий пришёл раньше первого повтора: чужой итог ему попасться не должен.
    harness.start(Scenario::Lifecycle);
    harness.idle_for(Duration::from_millis(300));
    assert!(
        harness.repeats().is_empty(),
        "повторы после нового сценария: {:?}",
        harness.repeats()
    );
    assert_eq!(harness.output.single_result()["scenario"], "finish-on-tick");
}

#[test]
fn without_a_result_nothing_is_repeated() {
    let mut harness = harness(echo_config(3, 5));
    harness.idle_for(Duration::from_millis(100));
    assert!(harness.repeats().is_empty());
}

#[test]
fn zero_repeats_keep_a_single_result_line() {
    let mut harness = harness(echo_config(0, 5));
    harness.starter.start(Box::new(FinishOnTickScenario)).expect("старт");
    harness.run_until_result();
    harness.idle_for(Duration::from_millis(100));
    assert!(harness.repeats().is_empty());
}

#[test]
fn logcat_output_repeats_only_after_a_result() {
    let mut output = LogcatScenarioOutput::default();
    assert_eq!(output.repeat_last_result(), RepeatOutcome::NothingToRepeat);
    output.result(ScenarioReport {
        scenario: "hello".to_owned(),
        outcome: ScenarioOutcome::Passed {
            report: serde_json::json!({}),
        },
    });
    assert_eq!(output.repeat_last_result(), RepeatOutcome::Repeated);
}
