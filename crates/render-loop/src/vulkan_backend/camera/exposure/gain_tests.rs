//! AE потока рендера без GPU: холодный старт и расписание статистики, остаточная подтяжка, диагностика.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::auto_exposure::{AppliedExposure, ProductReach, ProposalConvergence};
use camera_contract::control::ExposureBias;
use camera_contract::three_a::AeAfLock;

use super::test_support::*;
use super::*;

// ---- холодный старт и расписание статистики ----

#[test]
fn statistics_are_demanded_every_frame_only_until_the_first_estimate() {
    let mut harness = Harness::new();
    let scene = uniform_scene(DARK_VALUE);
    assert_eq!(
        harness.step(Some(UNIT_EXPOSURE), &scene).statistics,
        StatisticsDemand::EveryFrame
    );
    // Первый замер принят на предыдущем шаге: дальше AE довольствуется периодом статистики кадра.
    assert_eq!(
        harness.step(Some(UNIT_EXPOSURE), &scene).statistics,
        StatisticsDemand::Periodic
    );
    // Новый тап — новая область, оценки по ней нет: снова «каждый кадр».
    harness.directives.publish(point_directive(0.75, 0.5, 1));
    assert_eq!(
        harness.step(Some(UNIT_EXPOSURE), &scene).statistics,
        StatisticsDemand::EveryFrame
    );
    assert_eq!(
        harness.step(Some(UNIT_EXPOSURE), &scene).statistics,
        StatisticsDemand::Periodic
    );
}

#[test]
fn the_cold_start_demand_is_bounded_when_no_estimate_ever_comes() {
    let config = AutoExposureConfig {
        cold_start_every_frame_limit: 5,
        ..AutoExposureConfig::default()
    };
    let mut harness = Harness::with_config(config);
    let scene = uniform_scene(DARK_VALUE);
    let demands: Vec<_> = (0..10).map(|_| harness.step(None, &scene).statistics).collect();
    assert_eq!(
        demands
            .iter()
            .filter(|demand| **demand == StatisticsDemand::EveryFrame)
            .count(),
        5
    );
    assert!(demands[5..].iter().all(|demand| *demand == StatisticsDemand::Periodic));
    assert_eq!(harness.runner.diagnostics().counters.cold_start_statistics_frames, 5);
}

#[test]
fn manual_exposure_never_demands_statistics_every_frame() {
    let mut harness = Harness::new();
    harness.directives.publish(ExposureDirective {
        authority: ExposureAuthority::Manual,
        ..ExposureDirective::DEFAULT
    });
    let demand = harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE)).statistics;
    assert_eq!(demand, StatisticsDemand::Periodic);
    assert!(harness.proposals.latest().is_none(), "в ручном режиме предложений нет");
}

// ---- остаточная подтяжка ----

fn applied(product: f64, shortfall_ev: f32) -> Option<AppliedExposure> {
    Some(AppliedExposure {
        product: ExposureProduct::from_seconds_iso(product).expect("произведение"),
        shortfall_ev,
        reach: ProductReach::Unbounded,
    })
}

#[test]
fn automatic_gain_closes_the_gap_between_the_target_and_the_frame_the_sensor_still_shows() {
    let mut harness = Harness::new();
    // Цель замера ≈ 3,99 с·ISO.
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let target_ev = required_product(DARK_VALUE).log2();
    harness.directives.publish(ExposureDirective {
        applied: applied(4.0, -0.1),
        bias: ExposureBias::from_ev(0.2).expect("компенсация"),
        ..ExposureDirective::DEFAULT
    });
    // Сенсор показывает 3,5 с·ISO: до цели с компенсацией (нехватка прошлого поколения не берётся) ещё ~0,39 EV.
    let plan = harness.step(Some(exposure_product(3.5)), &uniform_scene(DARK_VALUE));
    let expected_ev = target_ev + 0.2 - 3.5_f64.log2();
    assert!(
        // Подтяжка квантуется шагом 0,125 EV: ошибка не больше половины шага.
        (f64::from(plan.gain.ev()) - expected_ev).abs() < 0.0625 + 1e-6,
        "подтяжка {} EV против {expected_ev}",
        plan.gain.ev()
    );
    let diagnostics = harness.runner.diagnostics();
    assert_eq!(diagnostics.display_gain_ev, plan.gain.ev());
    // Диагностика называет кадр, для которого посчитана подтяжка: экран = его уровень + подтяжка.
    let shown = diagnostics.displayed_frame_log2_seconds_iso.expect("показанный кадр");
    assert!((shown - 3.5_f64.log2()).abs() < 1e-9, "{shown}");
}

#[test]
fn gain_is_capped_by_the_reach_of_the_directive_and_not_by_the_stale_shortfall() {
    let mut harness = Harness::new();
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    // Цель ≈ 3,99 с·ISO, а сенсор достаёт только до 2 с·ISO; нехватка в указании устарела (0).
    harness.directives.publish(ExposureDirective {
        applied: Some(AppliedExposure {
            product: ExposureProduct::from_seconds_iso(2.0).expect("произведение"),
            shortfall_ev: 0.0,
            reach: ProductReach::Between {
                darkest: ExposureProduct::from_seconds_iso(0.01).expect("произведение"),
                brightest: ExposureProduct::from_seconds_iso(2.0).expect("произведение"),
            },
        }),
        ..ExposureDirective::DEFAULT
    });
    let plan = harness.step(Some(exposure_product(2.0)), &uniform_scene(DARK_VALUE));
    assert!(
        plan.gain.ev().abs() < 1e-4,
        "на пределе сенсора подтяжки нет: {}",
        plan.gain.ev()
    );
}

#[test]
fn gain_is_limited_to_half_an_ev_in_both_directions() {
    let mut harness = Harness::new();
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    harness.directives.publish(ExposureDirective {
        applied: applied(1.0, 0.0),
        ..ExposureDirective::DEFAULT
    });
    // Кадр на 2 EV темнее цели → потолок +0,5 EV.
    let brighter = harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    assert!((brighter.gain.ev() - 0.5).abs() < 1e-4, "{}", brighter.gain.ev());
    // Кадр на ~2 EV светлее цели → пол −0,5 EV.
    let darker = harness.step(Some(exposure_product(16.0)), &uniform_scene(DARK_VALUE));
    assert!((darker.gain.ev() + 0.5).abs() < 1e-4, "{}", darker.gain.ev());
}

#[test]
fn manual_gain_pulls_the_frame_to_the_applied_product_and_unity_when_it_already_matches() {
    let mut harness = Harness::new();
    harness.directives.publish(ExposureDirective {
        authority: ExposureAuthority::Manual,
        applied: applied(2.0_f64.powf(0.3), 0.0),
        ..ExposureDirective::DEFAULT
    });
    let behind = harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    // 0,3 EV округляется до шага 1/32: ближайшее кратное — 10/32.
    assert!((behind.gain.ev() - 10.0 / 32.0).abs() < 1e-4, "{}", behind.gain.ev());
    let caught_up = harness.step(Some(exposure_product(2.0_f64.powf(0.3))), &uniform_scene(DARK_VALUE));
    assert_eq!(
        caught_up.gain,
        ExposureGain::UNITY,
        "кадр уже на нужной экспозиции: ровно 1"
    );
    let far_behind = harness.step(Some(exposure_product(0.1)), &uniform_scene(DARK_VALUE));
    assert!((far_behind.gain.ev() - 0.5).abs() < 1e-4, "ограничен 0,5 EV");
}

#[test]
fn gain_is_quantized_so_the_params_cache_keeps_hitting_and_tiny_residuals_are_exactly_unity() {
    let mut harness = Harness::new();
    let mut gain_for_applied_ev = |applied_ev: f64| {
        harness.directives.publish(ExposureDirective {
            authority: ExposureAuthority::Manual,
            applied: applied(applied_ev.exp2(), 0.0),
            ..ExposureDirective::DEFAULT
        });
        harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE)).gain
    };
    // Меньше полушага (1/64 EV) — ровно 1: старый путь бит-в-бит.
    assert_eq!(gain_for_applied_ev(0.012), ExposureGain::UNITY);
    // Дрожание в четвёртом знаке даёт одно и то же значение — ключ кэша не меняется.
    assert_eq!(gain_for_applied_ev(0.2001), gain_for_applied_ev(0.1998));
    assert!((gain_for_applied_ev(0.2).ev() - 6.0 / 32.0).abs() < 1e-4);
}

#[test]
fn quantization_never_pushes_the_gain_past_the_controller_limit() {
    // Шаг 0,3 EV крупнее половины предела: 0,46 EV округляется до 0,6, но предел — 0,5.
    let config = AutoExposureConfig {
        residual_gain_step_ev: 0.3,
        ..AutoExposureConfig::default()
    };
    let mut harness = Harness::with_config(config);
    harness.directives.publish(ExposureDirective {
        authority: ExposureAuthority::Manual,
        applied: applied(0.46_f64.exp2(), 0.0),
        ..ExposureDirective::DEFAULT
    });
    let plan = harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    assert!((plan.gain.ev() - 0.5).abs() < 1e-4, "{}", plan.gain.ev());
}

#[test]
fn gain_is_unity_when_there_is_nothing_honest_to_base_it_on() {
    let mut harness = Harness::new();
    // Нет указания с применённой экспозицией.
    assert_eq!(
        harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE)).gain,
        ExposureGain::UNITY
    );
    harness.directives.publish(ExposureDirective {
        applied: applied(4.0, 0.0),
        ..ExposureDirective::DEFAULT
    });
    // Указание есть, но у кадра нет выдержки: подставлять нечего.
    let stream = Arc::clone(&harness.stream);
    let unpaired = missing_frame(&stream, 50);
    assert_eq!(harness.runner.plan_frame(&unpaired).gain, ExposureGain::UNITY);
}

#[test]
fn a_locked_target_keeps_the_proposal_holding_and_the_gain_steady() {
    let mut harness = Harness::new();
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    harness.directives.publish(ExposureDirective {
        lock: AeAfLock::Locked,
        applied: applied(4.0, 0.0),
        ..ExposureDirective::DEFAULT
    });
    harness.steps(90, Some(exposure_product(3.5)), &uniform_scene(DARK_VALUE));
    let held = harness.proposals.latest().expect("предложение").1;
    assert_eq!(held.convergence, ProposalConvergence::Holding);
    // Сцена стала ярче на 1 EV, но цель заморожена: подтяжка и предложение прежние.
    let gain_before = harness
        .step(Some(exposure_product(3.5)), &uniform_scene(DARK_VALUE))
        .gain;
    let gain_after = harness
        .steps(30, Some(exposure_product(3.5)), &uniform_scene(DARK_VALUE * 2.0))
        .gain;
    assert_eq!(gain_before, gain_after);
    assert_eq!(harness.latest_target(), held.target.seconds_iso());
}

#[test]
fn diagnostics_report_generation_authority_and_counters() {
    let mut harness = Harness::new();
    let generation = harness.directives.publish(ExposureDirective {
        authority: ExposureAuthority::SemiAutomatic,
        ..ExposureDirective::DEFAULT
    });
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let diagnostics = harness.runner.diagnostics();
    assert_eq!(diagnostics.directive_generation, Some(generation.0));
    assert_eq!(diagnostics.authority, ExposureAuthority::SemiAutomatic);
    assert!(diagnostics.counters.accepted >= 2);
    assert_eq!(
        diagnostics.last_proposal.map(|proposal| proposal.target),
        Some(ExposureProduct::from_seconds_iso(harness.latest_target()).unwrap())
    );
    assert!(diagnostics.target_log2_seconds_iso.is_some());
}

/// Шум лога 12b: обновление только обратной связи `applied` (≈ 15 раз в секунду) — не смена решения AE и в info не
/// идёт; смена EV, блокировки или области — смена решения.
#[test]
fn only_a_changed_decision_counts_as_a_new_ae_directive_for_the_log() {
    let base = ExposureDirective {
        applied: Some(AppliedExposure {
            product: ExposureProduct::from_seconds_iso(0.01).expect("произведение"),
            shortfall_ev: 0.0,
            reach: ProductReach::Unbounded,
        }),
        ..ExposureDirective::DEFAULT
    };
    let feedback_only = ExposureDirective {
        applied: Some(AppliedExposure {
            product: ExposureProduct::from_seconds_iso(0.02).expect("произведение"),
            shortfall_ev: -0.3,
            reach: ProductReach::Unbounded,
        }),
        ..base
    };
    assert!(
        !changes_decision(&base, &feedback_only),
        "только `applied` — обратная связь"
    );
    let new_bias = ExposureDirective {
        bias: ExposureBias::from_ev(1.0).expect("EV"),
        ..base
    };
    assert!(changes_decision(&base, &new_bias), "EV — решение");
    let locked = ExposureDirective {
        lock: AeAfLock::Locked,
        ..base
    };
    assert!(changes_decision(&base, &locked), "блокировка — решение");
}
