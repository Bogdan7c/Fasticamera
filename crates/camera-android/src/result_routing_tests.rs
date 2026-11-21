use super::*;

fn slot(index: u8) -> StandbySlotId {
    StandbySlotId::new(index)
}

#[test]
fn primary_results_go_to_the_engine_and_failures_too() {
    let routing = ResultRouting::new(CaptureRole::Primary);
    assert_eq!(routing.route_completed(), CompletedRoute::Engine);
    assert_eq!(routing.route_failed(), FailedRoute::Engine);
}

#[test]
fn standby_announces_streaming_exactly_once_on_its_first_result() {
    let routing = ResultRouting::new(CaptureRole::Standby(slot(2)));
    assert_eq!(
        routing.route_completed(),
        CompletedRoute::Standby {
            slot: slot(2),
            announce_streaming: true
        }
    );
    for _ in 0..3 {
        assert_eq!(
            routing.route_completed(),
            CompletedRoute::Standby {
                slot: slot(2),
                announce_streaming: false
            }
        );
    }
}

#[test]
fn standby_failure_is_counted_per_slot_and_does_not_mark_first_result() {
    let routing = ResultRouting::new(CaptureRole::Standby(slot(1)));
    assert_eq!(routing.route_failed(), FailedRoute::Standby { slot: slot(1) });
    // Провал не доказал готовность: первый успешный результат всё ещё объявляет её.
    assert_eq!(
        routing.route_completed(),
        CompletedRoute::Standby {
            slot: slot(1),
            announce_streaming: true
        }
    );
}

#[test]
fn promotion_switches_delivery_to_the_engine_at_once() {
    let routing = ResultRouting::new(CaptureRole::Standby(slot(0)));
    let _ = routing.route_completed();
    routing.promote_to_primary();
    assert_eq!(routing.role(), CaptureRole::Primary);
    assert_eq!(routing.route_completed(), CompletedRoute::Engine);
    assert_eq!(routing.route_failed(), FailedRoute::Engine);
}

#[test]
fn demoted_primary_stops_reaching_the_engine_and_never_announces_streaming() {
    let routing = ResultRouting::new(CaptureRole::Primary);
    // Основная ещё не выдала ни одного результата, но движок считает её готовой сразу.
    routing.demote_to_standby(slot(0));
    assert_eq!(
        routing.route_completed(),
        CompletedRoute::Standby {
            slot: slot(0),
            announce_streaming: false
        }
    );
    assert_eq!(routing.route_failed(), FailedRoute::Standby { slot: slot(0) });
}

#[test]
fn a_session_that_streamed_as_primary_does_not_announce_after_demotion() {
    let routing = ResultRouting::new(CaptureRole::Primary);
    assert_eq!(routing.route_completed(), CompletedRoute::Engine);
    routing.demote_to_standby(slot(3));
    assert_eq!(routing.role(), CaptureRole::Standby(slot(3)));
    assert!(matches!(
        routing.route_completed(),
        CompletedRoute::Standby {
            announce_streaming: false,
            ..
        }
    ));
}

#[test]
fn role_round_trips_for_every_slot_number() {
    for index in [0_u8, 1, 7, 254] {
        let routing = ResultRouting::new(CaptureRole::Standby(slot(index)));
        assert_eq!(routing.role(), CaptureRole::Standby(slot(index)));
    }
}
