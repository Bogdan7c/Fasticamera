use camera_contract::three_a::{BurstId, PointRequestId};

use super::*;

fn trigger(request: u64) -> RequestOrigin {
    RequestOrigin::FocusTrigger {
        request: PointRequestId(request),
    }
}

#[test]
fn repeating_key_resolves_without_being_registered() {
    let table = RequestOriginTable::new(4);
    assert_eq!(
        table.resolve(RequestKey::REPEATING.as_raw()),
        OriginLookup::Known(RequestOrigin::Repeating)
    );
    assert_eq!(table.pending(), 0);
}

#[test]
fn null_context_is_not_mistaken_for_the_repeating_request() {
    let table = RequestOriginTable::new(4);
    let lookup = table.resolve(0);
    assert_eq!(lookup, OriginLookup::NoContext);
    assert_eq!(lookup.origin(), RequestOrigin::Unknown);
}

#[test]
fn registered_one_shot_requests_resolve_to_their_own_origin_with_distinct_keys() {
    let table = RequestOriginTable::new(8);
    let first = table.register(RequestOrigin::Burst {
        burst: BurstId(3),
        index: 0,
    });
    let second = table.register(RequestOrigin::Burst {
        burst: BurstId(3),
        index: 1,
    });
    let tap = table.register(trigger(9));
    assert_ne!(first.key, second.key);
    assert_ne!(second.key, tap.key);
    assert_eq!(
        table.resolve(second.key.as_raw()),
        OriginLookup::Known(RequestOrigin::Burst {
            burst: BurstId(3),
            index: 1
        })
    );
    assert_eq!(table.resolve(tap.key.as_raw()), OriginLookup::Known(trigger(9)));
    assert!(first.key.as_raw() >= FIRST_ONE_SHOT_KEY, "0 и 1 зарезервированы");
}

#[test]
fn settle_removes_only_that_request_and_a_second_settle_is_harmless() {
    let table = RequestOriginTable::new(8);
    let first = table.register(trigger(1));
    let second = table.register(trigger(2));
    table.settle(first.key.as_raw());
    assert_eq!(table.resolve(first.key.as_raw()), OriginLookup::Unregistered);
    assert_eq!(table.resolve(second.key.as_raw()), OriginLookup::Known(trigger(2)));
    table.settle(first.key.as_raw());
    table.settle(0);
    table.settle(RequestKey::REPEATING.as_raw());
    assert_eq!(table.pending(), 1, "лишние снятия ничего не задели");
}

#[test]
fn failed_submit_unregisters_the_key() {
    let table = RequestOriginTable::new(8);
    let registration = table.register(trigger(5));
    table.unregister(registration.key);
    assert_eq!(table.resolve(registration.key.as_raw()), OriginLookup::Unregistered);
    assert_eq!(table.pending(), 0);
}

#[test]
fn full_table_evicts_the_oldest_and_says_so() {
    let table = RequestOriginTable::new(2);
    let oldest = table.register(trigger(1));
    let middle = table.register(trigger(2));
    assert!(!oldest.evicted_oldest && !middle.evicted_oldest);
    let newest = table.register(trigger(3));
    assert!(newest.evicted_oldest);
    assert_eq!(table.resolve(oldest.key.as_raw()), OriginLookup::Unregistered);
    assert_eq!(table.resolve(oldest.key.as_raw()).origin(), RequestOrigin::Unknown);
    assert_eq!(table.resolve(middle.key.as_raw()), OriginLookup::Known(trigger(2)));
    assert_eq!(table.resolve(newest.key.as_raw()), OriginLookup::Known(trigger(3)));
    assert_eq!(table.pending(), 2);
}

#[test]
fn unknown_key_is_unregistered_not_repeating() {
    let table = RequestOriginTable::new(2);
    assert_eq!(table.resolve(12345), OriginLookup::Unregistered);
}

#[test]
fn zero_capacity_still_keeps_the_latest_request() {
    let table = RequestOriginTable::new(0);
    let only = table.register(trigger(1));
    assert_eq!(table.resolve(only.key.as_raw()), OriginLookup::Known(trigger(1)));
}

#[test]
fn flash_still_origin_resolves_by_its_shot_and_is_settled_like_any_one_shot() {
    use camera_contract::flash::FlashShotId;

    let table = RequestOriginTable::new(4);
    let first = table.register(RequestOrigin::FlashStill { shot: FlashShotId(7) });
    let second = table.register(RequestOrigin::FlashStill { shot: FlashShotId(8) });
    assert_eq!(
        table.resolve(first.key.as_raw()),
        OriginLookup::Known(RequestOrigin::FlashStill { shot: FlashShotId(7) })
    );
    assert_eq!(
        table.resolve(second.key.as_raw()).origin(),
        RequestOrigin::FlashStill { shot: FlashShotId(8) }
    );
    table.settle(first.key.as_raw());
    assert_eq!(table.resolve(first.key.as_raw()), OriginLookup::Unregistered);
    assert_eq!(table.pending(), 1, "соседний снимок не задет");
}
