//! По тесту на каждое правило вердикта `3a`: здоровое наблюдение проходит, сломанное правило даёт ровно одно
//! нарушение с нужным текстом.

use camera_contract::three_a::{AeState, AfState};

use super::*;
use crate::scenario::camera::engine_watch::RunFailure;
use crate::scenario::camera::three_a_own_ae::{AeSource, EngineAeCount, OwnAeReport, ResultsChecked};
use crate::scenario::camera::three_a_report::{
    AppliedExposure, AwbVsHal, BracketFrameReport, EvPointReport, LongShutterReport, TapReport, ThreeAObservation,
};
use crate::scenario::camera::viewfinder_session::SessionSummary;

fn config() -> ThreeAScenarioConfig {
    ThreeAScenarioConfig::default()
}

fn tap(result: TapResult) -> TapReport {
    TapReport {
        x: 0.5,
        y: 0.5,
        result,
        frames_to_settle: Some(12),
        diopters_before: Some(1.0),
        diopters_after: Some(2.0),
    }
}

/// Здоровый прогон на телефоне: всё по плану.
fn healthy() -> ThreeAObservation {
    let mut observation = ThreeAObservation::new(SessionSummary {
        configured: None,
        degraded: None,
        time_to_streaming_ms: Some(900),
        time_to_live_ms: Some(1_100),
        state_at_end: "streaming",
        events: Vec::new(),
    });
    observation.live = Some(LiveReport {
        ae_source: AeSource::Vendor,
        companion: CompanionStreamStatus::Active {
            width: 640,
            height: 480,
        },
        af_state: Some(AfState::PassiveFocused),
        ae_state: Some(AeState::Converged),
    });
    observation.awb = Some(AwbReport {
        ready: true,
        hidden_since_start: 3,
        forced_since_start: 0,
        max_hidden_frames: 10,
        awb_vs_hal: AwbVsHal::Ok,
        white_balance: None,
    });
    observation.focus = Some(FocusReport {
        taps: vec![tap(TapResult::Focused); 5],
        region_effect_diopters: Some(0.4),
        triggers_sent: 5,
        results_without_request_context: 0,
        results_with_unknown_origin: 0,
    });
    observation.manual = Some(ManualReport {
        requested_exposure_ns: 1_000_000,
        requested_iso: 400,
        applied: Some(AppliedExposure {
            exposure_ns: 996_220,
            iso: 400,
            latency_frames: 6,
        }),
        adjustments_for_in_range: 0,
        long_shutter: LongShutterReport {
            requested_ns: 100_000_000,
            frame_budget_ns: Some(16_666_667),
            limited_event_seen: true,
            applied_ns: Some(16_600_000),
        },
    });
    observation.ev = Some(EvReport {
        ae_source: AeSource::Vendor,
        points: vec![
            ev_point(0.0, 1.0, true),
            ev_point(1.0, 2.0, true),
            ev_point(-1.0, 0.5, true),
        ],
        ev_unverifiable_at_limit: false,
    });
    observation.lock = Some(LockReport {
        ae_source: AeSource::Vendor,
        locked: true,
        samples_held: 40,
        non_locked_after_lock: 0,
        max_product_deviation: Some(0.0),
        observed_scene_range_ev: None,
        product_frozen_after_frames: None,
        prompt_prepare: None,
        prompt_hold_start: None,
        prompt_hold_end: None,
        back_to_continuous: true,
    });
    observation.bracket = Some(BracketRunReport {
        captured: Some(true),
        failure: None,
        frames: [
            (0, 100, 2_000_000),
            (1, 101, 4_000_000),
            (2, 102, 8_000_000),
            (3, 103, 16_000_000),
        ]
        .into_iter()
        .map(|(index, frame_number, exposure_ns)| BracketFrameReport {
            index,
            frame_number,
            exposure_ns: Some(exposure_ns),
            iso: Some(400),
        })
        .collect(),
        frame_gaps: 0,
        skipped_delta: 4,
        dropped_during: 0,
        presented_during: 30,
    });
    observation
}

fn ev_point(bias_ev: f32, ratio: f64, verifiable: bool) -> EvPointReport {
    EvPointReport {
        bias_ev,
        median_product: Some(ratio * 1.0e9),
        samples: 20,
        ae_converged: true,
        ae_limit_reached: false,
        verifiable,
        ratio_to_baseline: Some(ratio),
    }
}

fn violations_of(change: impl FnOnce(&mut ThreeAObservation)) -> Vec<String> {
    let mut observation = healthy();
    change(&mut observation);
    evaluate_three_a(&config(), &observation)
}

fn assert_single(violations: &[String], needle: &str) {
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains(needle), "нет «{needle}» в {violations:?}");
}

#[test]
fn healthy_run_passes() {
    assert_eq!(violations_of(|_| {}), Vec::<String>::new());
}

#[test]
fn run_failure_is_reported_first() {
    let violations = violations_of(|o| {
        o.failure = Some(RunFailure::EngineFatal {
            cause: "сбой".to_owned(),
        });
    });
    assert!(violations[0].contains("сбой"), "{violations:?}");
}

#[test]
fn dead_companion_missing_af_and_missing_ae_are_three_separate_violations() {
    let dead = violations_of(|o| {
        if let Some(live) = &mut o.live {
            live.companion = CompanionStreamStatus::DisabledByConfig;
        }
    });
    assert_single(&dead, "вспомогательный поток 3A не активен");
    let no_af = violations_of(|o| o.live.iter_mut().for_each(|live| live.af_state = None));
    assert_single(&no_af, "нет AF_STATE");
    let no_ae = violations_of(|o| o.live.iter_mut().for_each(|live| live.ae_state = None));
    assert_single(&no_ae, "нет AE_STATE");
}

#[test]
fn awb_not_ready_too_many_hidden_frames_and_forced_show_each_fail() {
    assert_single(
        &violations_of(|o| o.awb.iter_mut().for_each(|a| a.ready = false)),
        "AWB не готов",
    );
    assert_single(
        &violations_of(|o| o.awb.iter_mut().for_each(|a| a.hidden_since_start = 11)),
        "скрыл 11 кадров, предел 10",
    );
    assert_single(
        &violations_of(|o| o.awb.iter_mut().for_each(|a| a.forced_since_start = 1)),
        "принудительным показом",
    );
}

#[test]
fn awb_far_from_hal_is_a_note_not_a_violation() {
    let violations = violations_of(|o| o.awb.iter_mut().for_each(|a| a.awb_vs_hal = AwbVsHal::Warn));
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn center_tap_must_settle_and_not_time_out_or_be_undelivered() {
    let timed_out = violations_of(|o| o.focus.iter_mut().for_each(|f| f.taps[0].result = TapResult::TimedOut));
    assert_single(&timed_out, "тап в центр: HAL не сообщил итог");
    let not_delivered = violations_of(|o| {
        o.focus
            .iter_mut()
            .for_each(|f| f.taps[0].result = TapResult::TriggerNotDelivered);
    });
    assert_single(&not_delivered, "бэкенд отверг разовый запрос");
    let no_event = violations_of(|o| o.focus.iter_mut().for_each(|f| f.taps[0].result = TapResult::NoEvent));
    assert_single(&no_event, "событие итога не пришло");
    let not_focused = violations_of(|o| {
        o.focus
            .iter_mut()
            .for_each(|f| f.taps[0].result = TapResult::NotFocused)
    });
    assert!(not_focused.is_empty(), "NotFocused — честный итог HAL: {not_focused:?}");
}

#[test]
fn corner_tap_timeout_is_a_note_but_an_undelivered_corner_trigger_fails() {
    let timed_out = violations_of(|o| o.focus.iter_mut().for_each(|f| f.taps[3].result = TapResult::TimedOut));
    assert!(timed_out.is_empty(), "{timed_out:?}");
    let undelivered = violations_of(|o| {
        o.focus
            .iter_mut()
            .for_each(|f| f.taps[3].result = TapResult::TriggerNotDelivered);
    });
    assert_single(&undelivered, "тап 3");
}

#[test]
fn lost_request_context_unknown_origin_and_missing_triggers_each_fail() {
    assert_single(
        &violations_of(|o| o.focus.iter_mut().for_each(|f| f.results_without_request_context = 2)),
        "без контекста запроса: 2",
    );
    assert_single(
        &violations_of(|o| o.focus.iter_mut().for_each(|f| f.results_with_unknown_origin = 1)),
        "неопознанным происхождением: 1",
    );
    assert_single(
        &violations_of(|o| o.focus.iter_mut().for_each(|f| f.triggers_sent = 4)),
        "отправлено триггеров 4, тапов 5",
    );
}

#[test]
fn manual_exposure_and_iso_must_match_within_tolerance() {
    // 1,5 % при допуске 1 %.
    let off_exposure = violations_of(|o| {
        o.manual.iter_mut().for_each(|m| {
            m.applied = m.applied.map(|a| AppliedExposure {
                exposure_ns: 1_015_000,
                ..a
            })
        });
    });
    assert_single(&off_exposure, "выдержка в результате 1015000");
    let off_iso = violations_of(|o| {
        o.manual
            .iter_mut()
            .for_each(|m| m.applied = m.applied.map(|a| AppliedExposure { iso: 800, ..a }));
    });
    assert_single(&off_iso, "ISO в результате 800");
    let never = violations_of(|o| o.manual.iter_mut().for_each(|m| m.applied = None));
    assert_single(&never, "не появилась в результатах");
}

#[test]
fn in_range_manual_values_must_not_raise_adjustments() {
    let violations = violations_of(|o| o.manual.iter_mut().for_each(|m| m.adjustments_for_in_range = 1));
    assert_single(&violations, "событий ControlsAdjusted (ожидалось 0)");
}

#[test]
fn too_long_shutter_needs_the_event_and_a_result_within_the_frame() {
    let no_event = violations_of(|o| {
        o.manual
            .iter_mut()
            .for_each(|m| m.long_shutter.limited_event_seen = false)
    });
    assert_single(&no_event, "LimitedByFrameDuration");
    let too_long = violations_of(|o| {
        o.manual
            .iter_mut()
            .for_each(|m| m.long_shutter.applied_ns = Some(30_000_000))
    });
    assert_single(&too_long, "больше длительности кадра");
    let none = violations_of(|o| o.manual.iter_mut().for_each(|m| m.long_shutter.applied_ns = None));
    assert_single(&none, "не получен за срок");
}

#[test]
fn ev_that_does_not_double_fails_but_unverifiable_points_are_skipped() {
    let broken = violations_of(|o| {
        o.ev.iter_mut().for_each(|ev| ev.points[1] = ev_point(1.0, 1.05, true));
    });
    assert_single(&broken, "EV +1");
    let at_limit = violations_of(|o| {
        o.ev.iter_mut().for_each(|ev| {
            ev.points[1] = ev_point(1.0, 1.05, false);
            ev.ev_unverifiable_at_limit = true;
        });
    });
    assert!(at_limit.is_empty(), "{at_limit:?}");
    let no_median = violations_of(|o| {
        o.ev.iter_mut().for_each(|ev| ev.points[2].ratio_to_baseline = None);
    });
    assert_single(&no_median, "EV -1: нет медианы");
}

#[test]
fn lock_must_reach_locked_stay_constant_and_return_to_continuous() {
    assert_single(
        &violations_of(|o| o.lock.iter_mut().for_each(|l| l.locked = false)),
        "не стал Locked",
    );
    assert_single(
        &violations_of(|o| o.lock.iter_mut().for_each(|l| l.non_locked_after_lock = 2)),
        "2 результатов с AE_STATE не Locked",
    );
    assert_single(
        &violations_of(|o| o.lock.iter_mut().for_each(|l| l.max_product_deviation = Some(0.05))),
        "плавала на 5.0 %",
    );
    assert_single(
        &violations_of(|o| o.lock.iter_mut().for_each(|l| l.back_to_continuous = false)),
        "AF не вернулся в непрерывный режим",
    );
}

#[test]
fn bracket_needs_captured_four_ordered_consecutive_frames_with_requested_exposures() {
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.captured = None)),
        "итог серии (BracketCaptured) не пришёл",
    );
    assert_single(
        &violations_of(|o| {
            o.bracket.iter_mut().for_each(|b| {
                b.captured = Some(false);
                b.failure = Some("не все кадры".to_owned());
            });
        }),
        "серия не снята: не все кадры",
    );
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.frames.truncate(3))),
        "в серии 3 кадров, просили 4",
    );
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.frames[1].index = 5)),
        "не в порядке индексов",
    );
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.frame_gaps = 2)),
        "не подряд: пропущено 2",
    );
    assert_single(
        &violations_of(|o| {
            o.bracket
                .iter_mut()
                .for_each(|b| b.frames[2].exposure_ns = Some(9_000_000))
        }),
        "кадр 2 серии",
    );
}

#[test]
fn bracket_render_accounting_drops_and_presentation_are_checked() {
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.skipped_delta = 3)),
        "пропустил кадров серии 3",
    );
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.skipped_delta = 9)),
        "пропустил кадров серии 9",
    );
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.dropped_during = 3)),
        "потерял 3 кадров",
    );
    assert_single(
        &violations_of(|o| o.bracket.iter_mut().for_each(|b| b.presented_during = 0)),
        "не показал ни одного кадра",
    );
}

#[test]
fn failed_restore_is_a_violation() {
    let violations = violations_of(|o| o.restore_failure = Some("очередь полна".to_owned()));
    assert_single(&violations, "автоматические параметры не возвращены: очередь полна");
}

/// Худший провал: каждое правило нарушено, тексты причин длинные, все пять тапов и три точки EV в отчёте.
/// Строка результата обязана влезть в запись logcat и сохранить первое нарушение и ключевые числа.
#[test]
fn worst_case_failure_line_fits_the_logcat_record() {
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::scenario::camera::result_budget::worst_case::{
        assert_failure_line_fits, long_failure, long_reason, worst_session,
    };

    let mut observation = healthy();
    observation.session = worst_session();
    observation.failure = Some(long_failure());
    observation.restore_failure = Some(long_reason("возврат"));
    observation.live = Some(LiveReport {
        ae_source: AeSource::Vendor,
        companion: CompanionStreamStatus::Unavailable {
            reason: camera_android::companion_stream::CompanionUnavailableReason::ReaderCreationFailed {
                status: -10_000,
            },
        },
        af_state: None,
        ae_state: None,
    });
    observation.focus.iter_mut().for_each(|focus| {
        focus
            .taps
            .iter_mut()
            .for_each(|tap| tap.result = TapResult::TriggerNotDelivered);
        focus.results_without_request_context = 99;
        focus.results_with_unknown_origin = 99;
        focus.triggers_sent = 0;
    });
    observation.manual.iter_mut().for_each(|manual| {
        manual.applied = None;
        manual.adjustments_for_in_range = 9;
        manual.long_shutter.applied_ns = None;
        manual.long_shutter.limited_event_seen = false;
    });
    observation.bracket.iter_mut().for_each(|bracket| {
        bracket.captured = Some(false);
        bracket.failure = Some(long_reason("серия"));
        bracket.skipped_delta = 0;
        bracket.presented_during = 0;
    });
    let violations = evaluate_three_a(&config(), &observation);
    assert!(violations.len() >= 10, "{violations:?}");

    let verdict = judge_within_budget(observation.summary(), violations.clone());
    let line = assert_failure_line_fits("3a", verdict.outcome);
    let error = line["error"].as_str().expect("текст ошибки");
    assert!(
        error.starts_with(&truncate_for_compare(&violations[0])),
        "первое нарушение: {error}"
    );
    assert_eq!(line["report"]["focus"]["triggers_sent"], 0, "ключевые числа на месте");
    assert_eq!(
        verdict.violation_details.len(),
        violations.len(),
        "полный список — в деталях"
    );
}

/// Первое нарушение в тексте ошибки может быть укорочено: сравниваем по началу.
fn truncate_for_compare(text: &str) -> String {
    text.chars().take(40).collect()
}

#[test]
fn healthy_summary_is_far_below_the_line_budget() {
    let bytes = healthy().summary().to_string().len();
    assert!(bytes < 3_000, "сводка здорового прогона {bytes} байт");
}

fn own_ae_report() -> OwnAeReport {
    OwnAeReport {
        progress_at_end: Some(camera_core::three_a_status::AutoExposureProgress::Settled),
        settled_seen: true,
        holding_seen: true,
        limit_reached_seen: false,
        worst_shortfall_ev: None,
        target_not_reached_ticks: 0,
        results_checked: ResultsChecked {
            repeating: 120,
            ..ResultsChecked::default()
        },
        vendor_ae_results: 0,
        first_vendor_ae_frame: None,
        engine: Some(EngineAeCount {
            results_observed: 360,
            results_with_hal_ae_active: 0,
        }),
    }
}

/// Прогон с нашим AE: композиция поставила его, движок его ведёт, отчёты шагов помечены.
fn make_own(observation: &mut ThreeAObservation) {
    observation.expected_ae_source = AeSource::Own;
    observation.own_ae = Some(own_ae_report());
    observation
        .live
        .iter_mut()
        .for_each(|live| live.ae_source = AeSource::Own);
    observation.ev.iter_mut().for_each(|ev| ev.ae_source = AeSource::Own);
    observation
        .lock
        .iter_mut()
        .for_each(|lock| lock.ae_source = AeSource::Own);
}

#[test]
fn our_ae_does_not_need_hal_ae_state_but_vendor_ae_does() {
    let own = violations_of(|o| {
        make_own(o);
        o.live.iter_mut().for_each(|live| live.ae_state = None);
    });
    assert_eq!(own, Vec::<String>::new());
}

#[test]
fn healthy_run_with_our_ae_passes_and_a_vendor_request_among_results_fails() {
    assert_eq!(
        violations_of(|o| o.own_ae = Some(own_ae_report())),
        Vec::<String>::new()
    );
    let vendor_leak = violations_of(|o| {
        make_own(o);
        o.own_ae = Some(OwnAeReport {
            vendor_ae_results: 3,
            first_vendor_ae_frame: Some(1_042),
            ..own_ae_report()
        });
    });
    assert_single(&vendor_leak, "вендорским AE");
}

#[test]
fn vendor_ae_seen_only_by_the_engine_count_fails_and_a_missing_count_fails_too() {
    // Сценарий по своей выборке вендорского AE не видел, а движок по каждому результату насчитал.
    let unseen_leak = violations_of(|o| {
        make_own(o);
        o.own_ae = Some(OwnAeReport {
            engine: Some(EngineAeCount {
                results_observed: 360,
                results_with_hal_ae_active: 2,
            }),
            ..own_ae_report()
        });
    });
    assert_single(&unseen_leak, "движок с запуска насчитал 2");
    let no_count = violations_of(|o| {
        make_own(o);
        o.own_ae = Some(OwnAeReport {
            engine: None,
            ..own_ae_report()
        });
    });
    assert_single(&no_count, "нет счётчиков движка");
}

#[test]
fn our_ae_that_never_settled_holds_or_hits_a_limit_fails_but_a_limit_alone_is_honest() {
    let never = |o: &mut ThreeAObservation| {
        make_own(o);
        o.own_ae = Some(OwnAeReport {
            settled_seen: false,
            holding_seen: false,
            ..own_ae_report()
        });
    };
    assert_single(&violations_of(never), "не дошёл");
    let limit = violations_of(|o| {
        make_own(o);
        o.own_ae = Some(OwnAeReport {
            settled_seen: false,
            holding_seen: false,
            limit_reached_seen: true,
            ..own_ae_report()
        });
    });
    assert_eq!(limit, Vec::<String>::new());
}

#[test]
fn our_ae_lock_failures_name_holding_not_hal_locked() {
    let not_held = violations_of(|o| {
        make_own(o);
        o.lock.iter_mut().for_each(|l| l.locked = false);
    });
    assert_single(&not_held, "Holding");
    let left = violations_of(|o| {
        make_own(o);
        o.lock.iter_mut().for_each(|l| l.non_locked_after_lock = 4);
    });
    assert_single(&left, "вышел из Holding");
}

#[test]
fn engine_running_another_ae_than_the_composition_chose_fails_in_both_directions() {
    let configured_own_but_vendor_runs = violations_of(|o| {
        o.expected_ae_source = AeSource::Own;
        o.ae_fallback_reason = None;
    });
    assert_single(&configured_own_but_vendor_runs, "а композиция поставила Own");
    let chose_vendor_but_own_runs = violations_of(|o| {
        make_own(o);
        o.expected_ae_source = AeSource::Vendor;
        o.ae_fallback_reason = Some("рендер недоступен".to_owned());
        o.own_ae = None;
    });
    assert_single(&chose_vendor_but_own_runs, "замена: рендер недоступен");
}

#[test]
fn our_ae_ev_point_that_neither_settled_nor_hit_a_limit_fails() {
    let violations = violations_of(|o| {
        make_own(o);
        o.ev.iter_mut().for_each(|ev| ev.points[1].ae_converged = false);
    });
    assert_single(&violations, "EV +1: наш AE не сошёлся");
}

#[test]
fn our_ae_hitting_the_limit_at_every_shifted_point_means_ev_was_not_verified() {
    let all_limited = violations_of(|o| {
        make_own(o);
        o.ev.iter_mut().for_each(|ev| {
            for point in ev.points.iter_mut().filter(|point| point.bias_ev != 0.0) {
                point.ae_converged = false;
                point.ae_limit_reached = true;
                point.verifiable = false;
            }
        });
    });
    assert_single(&all_limited, "EV не проверен");
    let one_limited = violations_of(|o| {
        make_own(o);
        o.ev.iter_mut().for_each(|ev| {
            ev.points[1].ae_converged = false;
            ev.points[1].ae_limit_reached = true;
            ev.points[1].verifiable = false;
        });
    });
    assert_eq!(
        one_limited,
        Vec::<String>::new(),
        "один упор в предел — справка, не провал"
    );
}
