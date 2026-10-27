use super::*;

#[test]
fn offset_converts_a_monotonic_moment_to_the_same_moment_on_boottime() {
    // Телефон проспал 5 с: BOOTTIME ушёл вперёд на 5 с относительно MONOTONIC.
    let offset = ClockOffset::between(
        BoottimeNanos::from_nanos(105_000_000_000),
        MonotonicNanos::from_nanos(100_000_000_000),
    );
    let converted = offset.monotonic_to_boottime(MonotonicNanos::from_nanos(100_016_000_000));
    assert_eq!(converted, BoottimeNanos::from_nanos(105_016_000_000));
}

#[test]
fn offset_of_identical_clocks_changes_nothing() {
    let offset = ClockOffset::between(BoottimeNanos::from_nanos(42), MonotonicNanos::from_nanos(42));
    assert_eq!(
        offset.monotonic_to_boottime(MonotonicNanos::from_nanos(7)),
        BoottimeNanos::from_nanos(7)
    );
}

#[test]
fn system_clocks_give_a_plausible_offset_and_boottime_never_lags_monotonic() {
    let offset = SystemClocks.sample_offset().expect("системные часы читаются");
    // BOOTTIME включает время сна, поэтому не меньше MONOTONIC: смещение неотрицательно.
    let zero_on_monotonic = offset.monotonic_to_boottime(MonotonicNanos::from_nanos(0));
    assert!(zero_on_monotonic.as_nanos() >= 0, "{zero_on_monotonic:?}");
    // Два замера подряд дают то же смещение с точностью до миллисекунд (телефон не спал).
    let second = SystemClocks.sample_offset().expect("второй замер");
    let drift =
        (second.monotonic_to_boottime(MonotonicNanos::from_nanos(0)).as_nanos() - zero_on_monotonic.as_nanos()).abs();
    assert!(drift < 5_000_000, "смещение дрейфует на {drift} нс за один вызов");
}
