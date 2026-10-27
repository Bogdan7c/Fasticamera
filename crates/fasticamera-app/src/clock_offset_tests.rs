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
    // BOOTTIME включает время сна, поэтому не меньше MONOTONIC: истинное смещение ≥ 0. Но замер —
    // середина вилки чтений, его ошибка ± половина зазора (наносекунды–микросекунды). На машине,
    // которая с загрузки не спала, истинное смещение ровно 0, и замер законно выходит −5 нс (так
    // тест и падал в 07). Поэтому проверяем «не меньше минус погрешность», а не строгое «≥ 0».
    const MEASUREMENT_UNCERTAINTY_NANOS: i64 = 1_000_000;
    let zero_on_monotonic = offset.monotonic_to_boottime(MonotonicNanos::from_nanos(0));
    assert!(
        zero_on_monotonic.as_nanos() >= -MEASUREMENT_UNCERTAINTY_NANOS,
        "{zero_on_monotonic:?}"
    );
    // Два замера подряд дают то же смещение с точностью до миллисекунд (телефон не спал).
    let second = SystemClocks.sample_offset().expect("второй замер");
    let drift =
        (second.monotonic_to_boottime(MonotonicNanos::from_nanos(0)).as_nanos() - zero_on_monotonic.as_nanos()).abs();
    assert!(drift < 5_000_000, "смещение дрейфует на {drift} нс за один вызов");
}

fn bracket(monotonic_before: i64, boottime: i64, monotonic_after: i64) -> ClockBracket {
    ClockBracket {
        monotonic_before,
        boottime,
        monotonic_after,
    }
}

#[test]
fn preempted_bracket_is_ignored_in_favour_of_the_tightest_one() {
    // Истинное смещение 5 с. Первая вилка «вытеснена» на 8 мс между чтением BOOTTIME и вторым
    // MONOTONIC: её середина уезжает на 4 мс. Вторая вилка тесная (2 мкс) — смещение точное.
    let brackets = [
        bracket(1_000_000_000, 6_000_001_000, 1_008_000_000),
        bracket(2_000_000_000, 7_000_001_000, 2_000_002_000),
        bracket(3_000_000_000, 8_000_002_000, 3_000_006_000),
    ];
    let mut reads = brackets.into_iter();
    let offset = tightest_bracket_offset(3, || Ok(reads.next().expect("вилок хватает"))).unwrap();
    assert_eq!(
        offset.monotonic_to_boottime(MonotonicNanos::from_nanos(0)),
        BoottimeNanos::from_nanos(5_000_000_000),
        "выбрана вилка с зазором 2 мкс, а не первая вытесненная"
    );
}

#[test]
fn failed_read_in_any_bracket_fails_the_whole_sample() {
    let mut calls = 0;
    let result = tightest_bracket_offset(5, || {
        calls += 1;
        if calls == 3 {
            Err(ClockReadError {
                clock: "CLOCK_BOOTTIME",
                errno: 22,
            })
        } else {
            Ok(bracket(0, 10, 2))
        }
    });
    assert_eq!(
        result,
        Err(ClockReadError {
            clock: "CLOCK_BOOTTIME",
            errno: 22
        })
    );
    assert_eq!(calls, 3, "после ошибки чтения дальше не читаем");
}
