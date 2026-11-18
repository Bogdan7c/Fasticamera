//! Расписание повторов строки результата: число, интервал, отмена, конфиг.

use std::time::{Duration, Instant};

use super::*;

fn config(repeats: u32, interval_ms: u64) -> ResultEchoConfig {
    ResultEchoConfig { repeats, interval_ms }
}

/// Прогоняет расписание тиками по `tick` от `start` до `until` и возвращает моменты повторов.
fn repeats_between(echo: &mut ResultEcho, start: Instant, tick: Duration, until: Duration) -> Vec<Duration> {
    let mut moments = Vec::new();
    let mut now = start;
    while now <= start + until {
        if echo.take_due(now) == EchoDue::Repeat {
            moments.push(now - start);
        }
        now += tick;
    }
    moments
}

#[test]
fn without_a_result_there_is_nothing_to_repeat() {
    let mut echo = ResultEcho::new(ResultEchoConfig::default());
    assert_eq!(echo.take_due(Instant::now()), EchoDue::Idle);
}

#[test]
fn armed_echo_repeats_exactly_the_configured_number_of_times_spaced_by_the_interval() {
    let start = Instant::now();
    let mut echo = ResultEcho::new(config(3, 1000));
    echo.arm(start);

    let moments = repeats_between(&mut echo, start, Duration::from_millis(50), Duration::from_secs(10));

    // Ровно 3 копии, через 1 с, 2 с, 3 с: не раньше интервала и не пачкой.
    assert_eq!(
        moments,
        vec![
            Duration::from_millis(1000),
            Duration::from_millis(2000),
            Duration::from_millis(3000)
        ]
    );
    assert_eq!(echo.take_due(start + Duration::from_secs(60)), EchoDue::Idle);
}

#[test]
fn before_the_interval_it_is_not_yet_time() {
    let start = Instant::now();
    let mut echo = ResultEcho::new(config(2, 1000));
    echo.arm(start);
    assert_eq!(echo.take_due(start), EchoDue::NotYet);
    assert_eq!(echo.take_due(start + Duration::from_millis(999)), EchoDue::NotYet);
    assert_eq!(echo.take_due(start + Duration::from_millis(1000)), EchoDue::Repeat);
}

#[test]
fn a_late_tick_does_not_release_a_burst_of_copies() {
    let start = Instant::now();
    let mut echo = ResultEcho::new(config(3, 1000));
    echo.arm(start);
    // Маршрутизатор «проспал» 5 с: одна копия сейчас, следующая — только через интервал.
    let late = start + Duration::from_secs(5);
    assert_eq!(echo.take_due(late), EchoDue::Repeat);
    assert_eq!(echo.take_due(late), EchoDue::NotYet);
    assert_eq!(echo.take_due(late + Duration::from_millis(999)), EchoDue::NotYet);
    assert_eq!(echo.take_due(late + Duration::from_millis(1000)), EchoDue::Repeat);
}

#[test]
fn cancel_stops_pending_repeats() {
    let start = Instant::now();
    let mut echo = ResultEcho::new(config(5, 1000));
    echo.arm(start);
    assert_eq!(echo.take_due(start + Duration::from_secs(1)), EchoDue::Repeat);
    echo.cancel();
    let moments = repeats_between(&mut echo, start, Duration::from_millis(50), Duration::from_secs(10));
    assert!(moments.is_empty(), "после отмены повторов быть не должно: {moments:?}");
}

#[test]
fn rearming_replaces_the_previous_schedule() {
    let start = Instant::now();
    let mut echo = ResultEcho::new(config(2, 1000));
    echo.arm(start);
    assert_eq!(echo.take_due(start + Duration::from_secs(1)), EchoDue::Repeat);
    // Новый итог: счётчик повторов снова полный, срок — от нового итога.
    let second = start + Duration::from_millis(1500);
    echo.arm(second);
    let moments = repeats_between(&mut echo, second, Duration::from_millis(50), Duration::from_secs(10));
    assert_eq!(moments, vec![Duration::from_millis(1000), Duration::from_millis(2000)]);
}

#[test]
fn zero_repeats_disable_the_echo() {
    let start = Instant::now();
    let mut echo = ResultEcho::new(config(0, 1000));
    echo.arm(start);
    assert_eq!(echo.take_due(start + Duration::from_secs(5)), EchoDue::Idle);
    assert_eq!(config(0, 0).validate(), Ok(()), "без повторов интервал не важен");
}

#[test]
fn default_config_is_valid_and_zero_interval_with_repeats_is_rejected() {
    assert_eq!(ResultEchoConfig::default().validate(), Ok(()));
    assert_eq!(ResultEchoConfig::default(), config(5, 1000));
    assert_eq!(config(3, 0).validate(), Err(ResultEchoConfigError::ZeroInterval));
}
