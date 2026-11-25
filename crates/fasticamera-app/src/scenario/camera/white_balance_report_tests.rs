use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{
    HeldWhite, WhiteConfidence, WhiteOrigin, WhiteRequestId, WhiteSourceFailure, WhiteSourceId,
};
use camera_core::white::{WhiteCancelCause, WhiteRequestOutcome, WhiteRequestResult};
use render_loop::viewfinder::WhiteDelta;

use super::*;

fn setting(kelvin: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::from_duv(duv).expect("оттенок"),
    }
}

/// Статус владельца: белая 3200 К от телефона, заявка закрыта с этим исходом.
fn phone_status(outcome: WhiteRequestOutcome) -> WhiteStatus {
    let mut status = WhiteStatus::initial();
    status.held = HeldWhite {
        setting: setting(3_200.4, 0.003_456_7),
        origin: WhiteOrigin::Source(WhiteSourceId::PHONE),
    };
    status.request = WhiteRequestState::Idle {
        last: Some(WhiteRequestResult {
            request: WhiteRequestId(7),
            source: WhiteSourceId::PHONE,
            result: outcome,
        }),
    };
    status
}

fn applied() -> WhiteRequestOutcome {
    WhiteRequestOutcome::Applied {
        confidence: WhiteConfidence::FULL,
    }
}

#[test]
fn a_fresh_engine_reports_the_fallback_light_and_no_requests() {
    let report = WhiteBalanceReport::of(&WhiteBalanceDiagnostics::idle(), &WhiteStatus::initial(), 20);
    assert_eq!(report.origin, "fallback");
    assert_eq!(report.white.kelvin, 5_600.0);
    assert_eq!(report.request.state, "none");
    assert_eq!(report.request.id, None);
    assert_eq!(report.applied, None, "рендер указаний ещё не получил");
    assert!(!report.cold_start_pending);
}

#[test]
fn phone_white_reports_owner_value_origin_request_outcome_render_state_and_hal_reference() {
    let mut diagnostics = WhiteBalanceDiagnostics::idle();
    diagnostics.applied_white = Some(WhiteReading {
        temperature_kelvin: 3_199.6,
        duv: 0.003_456_7,
    });
    diagnostics.hal_delta = Some(WhiteDelta {
        mired: -12.345,
        duv: 0.001_23,
    });
    diagnostics.hidden_frames_cold_start = 3;
    diagnostics.cold_start_forced = 1;
    diagnostics.cold_start = ColdStartState::Pending { hidden: 2 };
    let mut owner = phone_status(applied());
    owner.stale_answers = 2;

    let report = WhiteBalanceReport::of(&diagnostics, &owner, 20);

    assert_eq!(
        report.white,
        WhitePointReport {
            kelvin: 3_200.0,
            duv: 0.0035
        }
    );
    assert_eq!(report.origin, "phone");
    assert_eq!(
        report.applied,
        Some(WhitePointReport {
            kelvin: 3_200.0,
            duv: 0.0035
        })
    );
    assert_eq!(
        report.request,
        WhiteRequestReport {
            state: "applied",
            id: Some(7),
            source: Some("phone")
        }
    );
    assert!(report.request_applied());
    assert_eq!(report.hal_delta_mired, Some(-12.35));
    assert_eq!(report.hal_delta_duv, Some(0.0012));
    assert!(report.cold_start_pending);
    assert_eq!((report.hidden_frames, report.cold_start_forced), (3, 1));
    assert_eq!(report.max_hidden_frames, 20);
    assert_eq!(report.stale_answers, 2);
}

#[test]
fn every_request_outcome_has_a_distinct_name_in_the_report() {
    let names: Vec<&str> = [
        applied(),
        WhiteRequestOutcome::Busy,
        WhiteRequestOutcome::Unknown {
            reason: camera_contract::white::WhiteUnknownReason::NoFreshMeasurement,
        },
        WhiteRequestOutcome::Unavailable {
            reason: camera_contract::white::WhiteUnavailableReason::AutomationSilent,
        },
        WhiteRequestOutcome::Failed {
            failure: WhiteSourceFailure::Executor,
        },
        WhiteRequestOutcome::NotUnderstood,
        WhiteRequestOutcome::TimedOut,
        WhiteRequestOutcome::Cancelled {
            by: WhiteCancelCause::ManualWhite,
        },
    ]
    .into_iter()
    .map(|outcome| {
        WhiteBalanceReport::of(&WhiteBalanceDiagnostics::idle(), &phone_status(outcome), 20)
            .request
            .state
    })
    .collect();
    assert_eq!(
        names,
        [
            "applied",
            "busy",
            "unknown",
            "unavailable",
            "failed",
            "not_understood",
            "timed_out",
            "cancelled"
        ]
    );
}

#[test]
fn a_waiting_request_is_reported_as_waiting_and_not_as_applied() {
    let mut owner = WhiteStatus::initial();
    owner.request = WhiteRequestState::Waiting {
        request: WhiteRequestId(3),
        source: WhiteSourceId::PHONE,
    };
    let report = WhiteBalanceReport::of(&WhiteBalanceDiagnostics::idle(), &owner, 20);
    assert_eq!(report.request.state, "waiting");
    assert_eq!(report.request.id, Some(3));
    assert!(!report.request_applied());
}

#[test]
fn verdict_accepts_phone_white_with_applied_request_and_rejects_everything_else() {
    let good = WhiteBalanceReport::of(&WhiteBalanceDiagnostics::idle(), &phone_status(applied()), 20);
    assert!(white_taken_from_phone_violations(&good).is_empty());

    // Запасной свет: телефон не ответил.
    let fallback = WhiteBalanceReport::of(&WhiteBalanceDiagnostics::idle(), &WhiteStatus::initial(), 20);
    let violations = white_taken_from_phone_violations(&fallback);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("белая не взята у телефона"), "{violations:?}");

    // Белая телефонная, но заявка закончилась отказом.
    let refused = WhiteBalanceReport::of(
        &WhiteBalanceDiagnostics::idle(),
        &phone_status(WhiteRequestOutcome::TimedOut),
        20,
    );
    assert!(
        white_taken_from_phone_violations(&refused)[0].contains("не закрыта исходом applied"),
        "{refused:?}"
    );

    // Холодный старт не кончился.
    let mut hidden = WhiteBalanceDiagnostics::idle();
    hidden.cold_start = ColdStartState::Pending { hidden: 1 };
    let pending = WhiteBalanceReport::of(&hidden, &phone_status(applied()), 20);
    let violations = white_taken_from_phone_violations(&pending);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("холодный старт не кончился"));
}

#[test]
fn hidden_frames_up_to_the_limit_pass_and_more_fail() {
    let mut report = WhiteBalanceReport::taken_from_phone_for_tests();
    report.hidden_frames = u64::from(report.max_hidden_frames);
    assert!(
        hidden_within_limit_violations(&report).is_empty(),
        "ровно предел — допустимо"
    );
    report.hidden_frames += 1;
    let violations = hidden_within_limit_violations(&report);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("скрыл 21 кадров, предел 20"), "{violations:?}");
}

#[test]
fn zero_counters_are_left_out_of_the_result_line_and_nonzero_ones_are_written() {
    let healthy = serde_json::to_value(WhiteBalanceReport::taken_from_phone_for_tests()).expect("JSON");
    assert!(
        healthy.get("cold_start_forced").is_none() && healthy.get("stale_answers").is_none(),
        "{healthy}"
    );

    let mut report = WhiteBalanceReport::taken_from_phone_for_tests();
    (report.cold_start_forced, report.stale_answers) = (2, 5);
    let value = serde_json::to_value(report).expect("JSON");
    assert_eq!(
        (&value["cold_start_forced"], &value["stale_answers"]),
        (&2.into(), &5.into())
    );
}
