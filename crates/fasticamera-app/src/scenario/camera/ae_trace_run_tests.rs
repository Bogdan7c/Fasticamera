//! Запись `ae-trace`: события расписания исполняются вовремя и один раз, профиль меняется на настоящем движке,
//! сигналы владельцу попадают в журнал с исходом платформы, ряд пишется не чаще заданного шага.

use std::sync::Mutex;
use std::time::Duration;

use camera_contract::three_a::ThreeAProfile;
use shell_contract::haptics::{HapticOutcome, HapticRealizationKind};

use super::super::ae_trace_test_support::diagnostics;
use super::*;
use crate::camera_test_support::{FakeCameraRig, WAIT};

/// Вибромотор-запись: помнит, что сыграли, и отвечает заданным исходом.
struct RecordingHaptics {
    played: Mutex<Vec<HapticPattern>>,
    outcome: HapticOutcome,
}

impl RecordingHaptics {
    fn accepting() -> Self {
        Self {
            played: Mutex::new(Vec::new()),
            outcome: HapticOutcome::Queued(HapticRealizationKind::Waveform),
        }
    }

    fn rejecting(outcome: HapticOutcome) -> Self {
        Self {
            played: Mutex::new(Vec::new()),
            outcome,
        }
    }

    fn played(&self) -> Vec<HapticPattern> {
        self.played.lock().expect("замок").clone()
    }
}

impl HapticsService for RecordingHaptics {
    fn play(&self, pattern: HapticPattern) -> HapticOutcome {
        self.played.lock().expect("замок").push(pattern);
        self.outcome
    }
}

fn config() -> AeTraceScenarioConfig {
    AeTraceScenarioConfig {
        photo_phase_ms: 1_600,
        video_phase_ms: 1_600,
        static_phase_ms: 800,
        cue_interval_ms: 800,
        static_warmup_ms: 100,
        sample_interval_ms: 50,
        step_window_ms: 300,
        series_capacity: 1_000,
        ..AeTraceScenarioConfig::default()
    }
}

/// Один шаг записи в момент `start + at_ms`.
fn step(
    tracing: &mut Tracing,
    config: &AeTraceScenarioConfig,
    start: Instant,
    at_ms: u64,
    rig: &FakeCameraRig,
    haptics: &RecordingHaptics,
) -> Result<Advance, RunFailure> {
    let handle = rig.handle();
    let diag = diagnostics(Some(1.0), Some(1.0), 0.0, 0, Some(0));
    let status = handle.status();
    tracing.advance(
        config,
        start + Duration::from_millis(at_ms),
        RecordingServices {
            handle: &handle,
            haptics,
        },
        Readings {
            diagnostics: &diag,
            three_a: &status.three_a,
        },
    )
}

fn profile_becomes(rig: &FakeCameraRig, expected: ThreeAProfile) -> bool {
    rig.handle()
        .wait_until_status(WAIT, |status| status.three_a.profile == expected)
        .is_some()
}

#[test]
fn phase_starts_switch_the_engine_profile_and_cues_play_a_single_tick() {
    let rig = FakeCameraRig::new();
    let config = config();
    let haptics = RecordingHaptics::accepting();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    assert_eq!(
        step(&mut tracing, &config, start, 0, &rig, &haptics),
        Ok(Advance::Continue)
    );
    assert_eq!(haptics.played(), vec![HapticPattern::LightImpact], "старт фото");
    assert!(profile_becomes(&rig, ThreeAProfile::Photo));

    step(&mut tracing, &config, start, 799, &rig, &haptics).expect("шаг");
    assert_eq!(haptics.played().len(), 1, "до подсказки ничего нового");
    step(&mut tracing, &config, start, 800, &rig, &haptics).expect("шаг");
    assert_eq!(
        haptics.played(),
        vec![HapticPattern::LightImpact, HapticPattern::SelectionTick]
    );

    step(&mut tracing, &config, start, 1_600, &rig, &haptics).expect("шаг");
    assert_eq!(
        haptics.played().last(),
        Some(&HapticPattern::MediumImpact),
        "старт видео"
    );
    assert!(
        profile_becomes(&rig, ThreeAProfile::Video),
        "движок получил профиль «Видео»"
    );

    // Повторный шаг в тот же момент не повторяет события.
    let before = haptics.played().len();
    step(&mut tracing, &config, start, 1_600, &rig, &haptics).expect("шаг");
    assert_eq!(haptics.played().len(), before);

    step(&mut tracing, &config, start, 3_200, &rig, &haptics).expect("шаг");
    assert_eq!(
        haptics.played().last(),
        Some(&HapticPattern::HeavyImpact),
        "старт static"
    );
    assert!(profile_becomes(&rig, ThreeAProfile::Photo), "static снова на «Фото»");
}

#[test]
fn schedule_finishes_exactly_at_the_end_and_the_outcome_carries_the_series() {
    let rig = FakeCameraRig::new();
    let config = config();
    let haptics = RecordingHaptics::accepting();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    assert_eq!(
        step(&mut tracing, &config, start, 3_950, &rig, &haptics),
        Ok(Advance::Continue)
    );
    assert_eq!(
        step(&mut tracing, &config, start, 4_000, &rig, &haptics),
        Ok(Advance::ScheduleFinished)
    );

    let outcome = tracing.finish();
    assert_eq!(outcome.timeline.total_ms(), 4_000);
    assert_eq!(outcome.series.len(), 2, "шаги через 50 мс — два отсчёта");
    assert_eq!(outcome.series[1].t_ms, 4_000);
    // Пропущенные события догнаны одним шагом: 3 старта фаз и 2 подсказки.
    assert_eq!(haptics.played().len(), 5);
}

#[test]
fn samples_are_written_no_more_often_than_the_configured_interval() {
    let rig = FakeCameraRig::new();
    let config = config();
    let haptics = RecordingHaptics::accepting();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    for at_ms in (0..=200).step_by(10) {
        step(&mut tracing, &config, start, at_ms, &rig, &haptics).expect("шаг");
    }

    let times: Vec<u64> = tracing.finish().series.iter().map(|sample| sample.t_ms).collect();
    assert_eq!(times, vec![0, 50, 100, 150, 200]);
}

#[test]
fn undelivered_signals_are_recorded_with_the_platform_outcome() {
    let rig = FakeCameraRig::new();
    let config = config();
    let haptics = RecordingHaptics::rejecting(HapticOutcome::NotAttached);
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    step(&mut tracing, &config, start, 800, &rig, &haptics).expect("шаг: сигнал не доставлен, но прогон идёт");

    let outcome = tracing.finish();
    assert_eq!(outcome.haptics.queued, 0);
    assert_eq!(outcome.haptics.failures.len(), 2, "старт фото и первая подсказка");
    assert!(
        outcome.haptics.failures[1].contains("подсказка 1 фазы photo"),
        "{:?}",
        outcome.haptics.failures
    );
    assert!(outcome.haptics.failures[1].contains("NotAttached"));
}

#[test]
fn ticks_without_diagnostics_are_counted_and_still_end_the_schedule() {
    let config = config();
    let start = Instant::now();
    let mut tracing = Tracing::begin(&config, start);

    assert_eq!(
        tracing.note_missing_diagnostics(start + Duration::from_millis(10)),
        Advance::Continue
    );
    assert_eq!(
        tracing.note_missing_diagnostics(start + Duration::from_millis(4_000)),
        Advance::ScheduleFinished,
        "расписание не должно зависнуть, если диагностика пропала на последних тиках"
    );
    assert_eq!(tracing.finish().ticks_without_ae, 2);
}
