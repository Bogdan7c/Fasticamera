use super::*;

fn stamp(nanos: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(nanos)
}

#[test]
fn result_gets_the_real_frame_number_by_timestamp_even_out_of_order() {
    let mut matcher = FrameNumberMatcher::new(4);
    matcher.record_started(stamp(1_000), FrameNumber(10));
    matcher.record_started(stamp(17_666), FrameNumber(11));
    matcher.record_started(stamp(34_333), FrameNumber(12));
    assert_eq!(
        matcher.resolve(Some(stamp(17_666))),
        FrameNumberSource::Reported(FrameNumber(11))
    );
    assert_eq!(
        matcher.resolve(Some(stamp(1_000))),
        FrameNumberSource::Reported(FrameNumber(10))
    );
    assert_eq!(
        matcher.resolve(Some(stamp(34_333))),
        FrameNumberSource::Reported(FrameNumber(12))
    );
}

#[test]
fn a_pair_is_consumed_so_a_repeated_timestamp_falls_back() {
    let mut matcher = FrameNumberMatcher::new(4);
    matcher.record_started(stamp(5), FrameNumber(1));
    assert_eq!(
        matcher.resolve(Some(stamp(5))),
        FrameNumberSource::Reported(FrameNumber(1))
    );
    assert_eq!(
        matcher.resolve(Some(stamp(5))),
        FrameNumberSource::Fallback(FrameNumber(-1))
    );
}

#[test]
fn unknown_or_missing_timestamp_uses_a_negative_fallback_that_is_marked_as_such() {
    let mut matcher = FrameNumberMatcher::new(4);
    assert_eq!(
        matcher.resolve(Some(stamp(99))),
        FrameNumberSource::Fallback(FrameNumber(-1))
    );
    assert_eq!(matcher.resolve(None), FrameNumberSource::Fallback(FrameNumber(-2)));
    assert_eq!(matcher.resolve(None).frame_number(), FrameNumber(-3));
}

#[test]
fn window_evicts_the_oldest_started_frame_when_results_lag_behind() {
    let mut matcher = FrameNumberMatcher::new(2);
    matcher.record_started(stamp(1), FrameNumber(1));
    matcher.record_started(stamp(2), FrameNumber(2));
    matcher.record_started(stamp(3), FrameNumber(3));
    assert!(
        matches!(matcher.resolve(Some(stamp(1))), FrameNumberSource::Fallback(_)),
        "кадр 1 вытеснен"
    );
    assert_eq!(
        matcher.resolve(Some(stamp(3))),
        FrameNumberSource::Reported(FrameNumber(3))
    );
    assert_eq!(
        matcher.resolve(Some(stamp(2))),
        FrameNumberSource::Reported(FrameNumber(2))
    );
}

#[test]
fn zero_capacity_is_raised_to_one_instead_of_dropping_every_pair() {
    let mut matcher = FrameNumberMatcher::new(0);
    matcher.record_started(stamp(7), FrameNumber(70));
    assert_eq!(
        matcher.resolve(Some(stamp(7))),
        FrameNumberSource::Reported(FrameNumber(70))
    );
}

#[test]
fn fallback_numbers_never_collide_with_real_ones_even_when_real_numbers_start_at_zero() {
    let mut matcher = FrameNumberMatcher::new(4);
    matcher.record_started(stamp(100), FrameNumber(0));
    let fallback = matcher.resolve(Some(stamp(999))).frame_number();
    let real = matcher.resolve(Some(stamp(100)));
    assert_eq!(real, FrameNumberSource::Reported(FrameNumber(0)));
    assert!(fallback.0 < 0, "запасной номер {fallback:?} должен быть отрицательным");
    assert_ne!(fallback, real.frame_number());
}
