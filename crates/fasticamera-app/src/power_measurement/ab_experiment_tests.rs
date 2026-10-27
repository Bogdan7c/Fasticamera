//! A/B-замер: синтетические потоки с известными токами → известные Δ, разброс и вывод.

use shell_contract::power::{BatteryCurrent, ChargingState, PowerPlug};

use super::*;
use crate::power_measurement::test_support::{at_ms, charging, on_battery, series, settings, settings_with_sign};

/// Окно 10 с, разогрев 2 с.
const WINDOW_MS: i64 = 10_000;
const WARMUP_MS: u64 = 2_000;

/// Показания всех окон: ток окна `index` даёт `current_of(index)`.
fn experiment_samples(windows: usize, current_of: impl Fn(usize) -> i64) -> Vec<BatterySample> {
    (0..windows)
        .flat_map(|index| {
            let from = i64::try_from(index).expect("индекс") * WINDOW_MS;
            series(from, from + WINDOW_MS, |_| current_of(index))
        })
        .collect()
}

/// Прогоняет показания одним тиком «после конца замера» и возвращает отчёт.
fn run(pairs: u32, current_of: impl Fn(usize) -> i64) -> AbPowerReport {
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, pairs), at_ms(0));
    let windows = experiment.total_windows();
    let samples = experiment_samples(windows, current_of);
    let end = i64::try_from(windows).expect("окна") * WINDOW_MS + 1_000;
    assert_eq!(experiment.on_tick(at_ms(end), &samples), AbStep::Finished);
    experiment.report().expect("отчёт").clone()
}

/// Ток окна: A (стадия включена) — `on_ma`, B — `off_ma`, плюс дрейф `drift_ma` на каждое окно.
fn stage_current(on_ma: i64, off_ma: i64, drift_ma: i64) -> impl Fn(usize) -> i64 {
    move |index| {
        let base = match AbPowerExperiment::stage_of(index) {
            StageSetting::On => on_ma,
            StageSetting::Off => off_ma,
        };
        base + drift_ma * i64::try_from(index).expect("индекс")
    }
}

#[test]
fn schedule_is_abba_and_pairs_alternate_their_order() {
    let stages: Vec<_> = (0..8).map(AbPowerExperiment::stage_of).collect();
    use StageSetting::{Off, On};
    assert_eq!(stages, [On, Off, Off, On, On, Off, Off, On]);
}

#[test]
fn known_currents_give_the_known_delta() {
    let report = run(3, stage_current(600, 500, 0));
    assert_eq!(
        report.contribution,
        Contribution::Measured {
            mean_delta_ma: 100.0,
            spread_ma: 0.0
        }
    );
    assert_eq!(report.discharge_sign, Some(DischargeSign::NegativeWhenDischarging));
    let orders: Vec<_> = report.pairs.iter().map(|pair| pair.order).collect();
    assert_eq!(orders, [PairOrder::AThenB, PairOrder::BThenA, PairOrder::AThenB]);
    assert!(
        report
            .pairs
            .iter()
            .all(|pair| pair.stage_on_ma == 600.0 && pair.stage_off_ma == 500.0)
    );
    assert_eq!(report.windows.len(), 6);
    assert!(report.windows.iter().all(|window| window.measurement.samples == 32));
}

#[test]
fn linear_drift_is_cancelled_exactly_with_an_even_number_of_pairs_and_shows_in_the_spread() {
    // Дрейф +12 мА на окно. При ABAB каждая пара дала бы 100 − 12 = 88 с нулевым разбросом —
    // уверенно неверно. ABBA: пары 88, 112, 88, 112.
    let report = run(4, stage_current(600, 500, 12));
    let deltas: Vec<_> = report.pairs.iter().map(|pair| pair.delta_ma).collect();
    assert_eq!(deltas, [88.0, 112.0, 88.0, 112.0]);
    assert_eq!(
        report.contribution,
        Contribution::Measured {
            mean_delta_ma: 100.0,
            spread_ma: 24.0
        }
    );
}

#[test]
fn odd_number_of_pairs_leaves_a_third_of_the_drift_and_reports_it_in_the_spread() {
    // Три пары: (88 + 112 + 88) / 3 = 96, остаток дрейфа d / N = 12 / 3 = 4 мА.
    let report = run(3, stage_current(600, 500, 12));
    assert_eq!(
        report.contribution,
        Contribution::Measured {
            mean_delta_ma: 96.0,
            spread_ma: 24.0
        }
    );
}

#[test]
fn spread_larger_than_the_delta_is_indistinguishable_without_a_number() {
    // Окна: A600 B500 | B560 A530 | A610 B500 → Δ = 100, −30, 110: среднее 60, разброс 140.
    let means = [600, 500, 560, 530, 610, 500];
    let report = run(3, |index| means[index]);
    assert_eq!(
        report.contribution,
        Contribution::Indistinguishable { spread_ma: 140.0 }
    );
    let json = serde_json::to_value(report.contribution).expect("json");
    assert_eq!(json["verdict"], "indistinguishable");
    assert!(json.get("mean_delta_ma").is_none(), "у «не различим» числа вклада нет");
}

#[test]
fn zero_delta_with_zero_spread_is_indistinguishable() {
    let report = run(3, stage_current(500, 500, 0));
    assert_eq!(report.contribution, Contribution::Indistinguishable { spread_ma: 0.0 });
}

#[test]
fn charging_sample_rejects_at_once_without_waiting_for_the_window_end() {
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(0));
    let current = stage_current(600, 500, 0);
    let mut samples = series(0, 35_000, |ms| current(usize::try_from(ms / WINDOW_MS).expect("окно")));
    samples.push(charging(35_000, 500));
    assert_eq!(experiment.on_tick(at_ms(35_100), &samples), AbStep::Finished);
    let report = experiment.report().expect("отчёт");
    assert_eq!(
        report.contribution,
        Contribution::Rejected {
            reason: AbRejection::Window {
                index: 3,
                stage: StageSetting::On,
                rejection: WindowRejection::ExternalPower {
                    plug: PowerPlug::Usb,
                    charging: ChargingState::Charging
                }
            }
        }
    );
    assert!(report.pairs.is_empty(), "при отказе Δ не считаются");
    assert_eq!(
        experiment.on_tick(at_ms(40_000), &[]),
        AbStep::Finished,
        "отказ окончателен"
    );
}

#[test]
fn ambiguous_sign_in_the_first_window_rejects_instead_of_guessing() {
    // Сырые токи окна 0 скачут через ноль: знак разряда не установить.
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(0));
    let samples = series(0, 10_000, |ms| if (ms / 250) % 2 == 0 { 500 } else { -500 });
    assert_eq!(experiment.on_tick(at_ms(11_000), &samples), AbStep::Finished);
    assert_eq!(
        experiment.report().expect("отчёт").contribution,
        Contribution::Rejected {
            reason: AbRejection::DischargeSignUndetermined {
                index: 0,
                evidence: DischargeSignEvidence::Ambiguous {
                    positive: 16,
                    negative: 16
                }
            }
        }
    );
}

#[test]
fn later_window_contradicting_the_established_sign_is_rejected() {
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(0));
    let mut samples = series(0, 10_000, |_| 500);
    // Окно 1: сырой ток стал положительным (как если бы пошла зарядка без смены статуса).
    samples.extend(series(10_000, 20_000, |_| -500));
    assert_eq!(experiment.on_tick(at_ms(21_000), &samples), AbStep::Finished);
    assert_eq!(
        experiment.report().expect("отчёт").contribution,
        Contribution::Rejected {
            reason: AbRejection::DischargeSignUndetermined {
                index: 1,
                evidence: DischargeSignEvidence::Detected {
                    sign: DischargeSign::PositiveWhenDischarging,
                    samples: 32,
                    opposing: 0
                }
            }
        }
    );
}

#[test]
fn configured_sign_is_used_as_is() {
    // Вендор с инвертированным знаком задан конфигом: сырое +600 000 мкА = разряд 600 мА.
    let mut experiment = AbPowerExperiment::new(
        &settings_with_sign(10_000, WARMUP_MS, 3, DischargeSign::PositiveWhenDischarging),
        at_ms(0),
    );
    let samples: Vec<_> = experiment_samples(6, stage_current(600, 500, 0))
        .into_iter()
        .map(|mut sample| {
            let raw = sample.current.expect("ток").raw_microamps();
            sample.current = Some(BatteryCurrent::from_raw_microamps(-raw));
            sample
        })
        .collect();
    assert_eq!(experiment.on_tick(at_ms(61_000), &samples), AbStep::Finished);
    assert_eq!(
        experiment.report().expect("отчёт").contribution,
        Contribution::Measured {
            mean_delta_ma: 100.0,
            spread_ma: 0.0
        }
    );
}

#[test]
fn on_tick_tells_the_scenario_which_stage_to_apply_and_when_warmup_ends() {
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(0));
    let step = |experiment: &mut AbPowerExperiment, ms: i64| experiment.on_tick(at_ms(ms), &[]);
    let measure = |stage, index: usize, warming_up| AbStep::Measure {
        stage,
        window: WindowPosition {
            index,
            pair: index / 2,
            warming_up,
        },
    };
    assert_eq!(step(&mut experiment, 0), measure(StageSetting::On, 0, true));
    assert_eq!(step(&mut experiment, 5_000), measure(StageSetting::On, 0, false));
    assert_eq!(step(&mut experiment, 10_100), measure(StageSetting::Off, 1, true));
}

#[test]
fn window_without_samples_is_rejected_as_missing_not_as_unknown_sign() {
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(0));
    assert_eq!(experiment.on_tick(at_ms(10_600), &[]), AbStep::Finished);
    assert_eq!(
        experiment.report().expect("отчёт").contribution,
        Contribution::Rejected {
            reason: AbRejection::Window {
                index: 0,
                stage: StageSetting::On,
                rejection: WindowRejection::TooManyMissingSamples {
                    missing: 32,
                    expected: 32
                }
            }
        }
    );
}

#[test]
fn samples_before_start_and_late_samples_are_counted_not_mixed_in() {
    // Замер начинается в 1000 мс: окно k — [1000 + k·10000, 1000 + (k+1)·10000).
    const START_MS: i64 = 1_000;
    let window_samples = |index: usize| {
        let from = START_MS + i64::try_from(index).expect("индекс") * WINDOW_MS;
        series(from, from + WINDOW_MS, |_| stage_current(600, 500, 0)(index))
    };
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(START_MS));
    let mut first = vec![on_battery(500, 9_999)];
    first.extend(window_samples(0));
    // Тик после конца окна 0 с запасом (11000 + 500): окно закрыто, идёт окно 1.
    assert!(matches!(
        experiment.on_tick(at_ms(11_600), &first),
        AbStep::Measure { .. }
    ));
    // Запоздавшее показание окна 0, затем все остальные окна.
    let mut rest = vec![on_battery(10_900, 9_999)];
    rest.extend((1..6).flat_map(window_samples));
    assert_eq!(experiment.on_tick(at_ms(62_000), &rest), AbStep::Finished);
    let report = experiment.report().expect("отчёт");
    assert_eq!(report.samples_before_start, 1);
    assert_eq!(report.late_samples, 1);
    assert_eq!(
        report.contribution,
        Contribution::Measured {
            mean_delta_ma: 100.0,
            spread_ma: 0.0
        },
        "9999 мА из чужого времени в средние не попали"
    );
}

/// Ядро в мА: весь A/B-замер отвергается в первом же окне как `UnitSuspect`, а не выдаёт Δ
/// в 1000 раз меньше настоящей.
#[test]
fn milliamp_kernel_rejects_the_experiment_instead_of_reporting_a_1000x_wrong_delta() {
    let mut experiment = AbPowerExperiment::new(&settings(10_000, WARMUP_MS, 3), at_ms(0));
    let windows = experiment.total_windows();
    let samples: Vec<_> = experiment_samples(windows, stage_current(600, 500, 0))
        .into_iter()
        .map(|mut sample| {
            // Сырое значение в мА: −600 000 мкА превращается в −600.
            sample.current = sample
                .current
                .map(|current| BatteryCurrent::from_raw_microamps(current.raw_microamps() / 1_000));
            sample
        })
        .collect();
    let end = i64::try_from(windows).expect("окна") * WINDOW_MS + 1_000;
    assert_eq!(experiment.on_tick(at_ms(end), &samples), AbStep::Finished);
    assert!(
        matches!(
            experiment.report().expect("отчёт").contribution,
            Contribution::Rejected {
                reason: AbRejection::Window {
                    index: 0,
                    rejection: WindowRejection::UnitSuspect { .. },
                    ..
                }
            }
        ),
        "{:?}",
        experiment.report()
    );
}

/// Одиночный шум другого знака в окне не срывает замер при пороге 95 %.
#[test]
fn rare_opposite_sign_noise_does_not_reject_the_experiment_at_95_percent_agreement() {
    use shell_contract::power::{SignAgreement, SignDetectionRule};

    let mut settings = settings(10_000, WARMUP_MS, 3);
    settings.sign = SignPolicy::Detect {
        rule: SignDetectionRule {
            required: std::num::NonZeroUsize::new(4).expect("не ноль"),
            agreement: SignAgreement::new(0.95).expect("доля"),
        },
    };
    let mut experiment = AbPowerExperiment::new(&settings, at_ms(0));
    let windows = experiment.total_windows();
    let mut samples = experiment_samples(windows, stage_current(600, 500, 0));
    // В каждом окне одно показание измеряемой части — шум +5 мА (сырое положительное).
    for index in 0..windows {
        let at = i64::try_from(index).expect("индекс") * WINDOW_MS + 5_000;
        if let Some(sample) = samples.iter_mut().find(|sample| sample.at == at_ms(at)) {
            sample.current = Some(BatteryCurrent::from_raw_microamps(5_000));
        }
    }
    let end = i64::try_from(windows).expect("окна") * WINDOW_MS + 1_000;
    assert_eq!(experiment.on_tick(at_ms(end), &samples), AbStep::Finished);
    let report = experiment.report().expect("отчёт");
    assert!(
        matches!(report.contribution, Contribution::Measured { .. }),
        "{:?}",
        report.contribution
    );
    assert_eq!(report.discharge_sign, Some(DischargeSign::NegativeWhenDischarging));
}
