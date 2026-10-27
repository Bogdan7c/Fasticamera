//! Ожидание до момента «позднего захвата» кадра камеры (сессия 07).
//!
//! Чем позже поток рендера берёт кадр камеры перед сроком сдачи композитору, тем свежее
//! картинка на экране. Но ждать надо ТОЧНО: сон ОС просыпается с опозданием (дрожание
//! планировщика — десятки–сотни мкс, под нагрузкой больше), и опоздание съедает запас до
//! срока линии кадра. Поэтому ожидание в два шага:
//! 1. пока до цели дальше `spin_window`, спим (CPU свободен);
//! 2. последние `spin_window` крутим активно, перечитывая часы, — точность в единицы мкс
//!    ценой занятого ядра на это время.
//!
//! Компромисс задаётся конфигом (`present_target.latch_spin_us`): больше окно — точнее, но
//! дороже по батарее и теплу; ноль — только сон, дешевле, но захват дрожит.
//!
//! Функция нейтральна: часы и сон — через [`MonotonicTimer`], тесты подставляют свои.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

use super::monotonic_clock::{ClockReadError, MonotonicTimer};

/// Что ждать и как.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SleepUntil {
    /// До какого момента ждать.
    pub target: MonotonicNanos,
    /// Последние столько ожидания — активно, без сна.
    pub spin_window: Duration,
    /// Если до цели дальше этого — не ждать вовсе (защита от абсурдного срока).
    pub max_wait: Duration,
}

/// Итог ожидания. Варианты различимы: «цель уже прошла», «дождались», «отказались ждать»,
/// «часы сломаны» — вызывающему важно каждое (счётчики и откаты разные).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepUntilOutcome {
    /// Цель уже прошла к моменту вызова: не ждали, опоздали на `late_by`.
    AlreadyPassed { late_by: Duration },
    /// Дождались; `overshoot` — насколько проснулись позже цели.
    Reached { overshoot: Duration },
    /// До цели дальше `max_wait`: не ждали (срок подозрителен).
    RefusedTooFar { remaining: Duration },
    /// Часы не прочитались: не ждали.
    ClockFailed(ClockReadError),
}

/// Ждёт до `plan.target` по часам `timer` (сон, затем активное ожидание последних
/// `spin_window`). Никогда не ждёт дольше `max_wait` от момента вызова.
pub fn sleep_until(plan: SleepUntil, timer: &impl MonotonicTimer) -> SleepUntilOutcome {
    let started = match timer.now() {
        Ok(now) => now,
        Err(error) => return SleepUntilOutcome::ClockFailed(error),
    };
    if started >= plan.target {
        return SleepUntilOutcome::AlreadyPassed {
            late_by: nanos_between(plan.target, started),
        };
    }
    let remaining = nanos_between(started, plan.target);
    if remaining > plan.max_wait {
        return SleepUntilOutcome::RefusedTooFar { remaining };
    }
    let mut now = started;
    while now < plan.target {
        let remaining = nanos_between(now, plan.target);
        if remaining > plan.spin_window {
            // Спим до начала окна активного ожидания, не до самой цели: проснуться позже цели
            // из-за дрожания сна — ровно то, чего избегаем.
            timer.sleep(remaining - plan.spin_window);
        } else {
            timer.spin_once();
        }
        now = match timer.now() {
            Ok(now) => now,
            Err(error) => return SleepUntilOutcome::ClockFailed(error),
        };
    }
    SleepUntilOutcome::Reached {
        overshoot: nanos_between(plan.target, now),
    }
}

/// Длительность от `earlier` до `later` (ноль, если порядок обратный).
fn nanos_between(earlier: MonotonicNanos, later: MonotonicNanos) -> Duration {
    let difference = later.as_nanos().saturating_sub(earlier.as_nanos());
    Duration::from_nanos(u64::try_from(difference).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// Поддельные часы: время двигают только сон и итерации ожидания.
    struct ScriptedTimer {
        now_nanos: Cell<i64>,
        /// Насколько сон ОС «проспал» сверх запрошенного (дрожание планировщика).
        sleep_oversleep_nanos: i64,
        /// Сколько стоит одна итерация активного ожидания.
        spin_step_nanos: i64,
        sleeps: RefCell<Vec<Duration>>,
        spins: Cell<u64>,
        fail_clock: bool,
    }

    impl ScriptedTimer {
        fn at(now_nanos: i64) -> Self {
            Self {
                now_nanos: Cell::new(now_nanos),
                sleep_oversleep_nanos: 0,
                spin_step_nanos: 1_000,
                sleeps: RefCell::new(Vec::new()),
                spins: Cell::new(0),
                fail_clock: false,
            }
        }
    }

    impl MonotonicTimer for ScriptedTimer {
        fn now(&self) -> Result<MonotonicNanos, ClockReadError> {
            if self.fail_clock {
                return Err(ClockReadError { errno: 22 });
            }
            Ok(MonotonicNanos::from_nanos(self.now_nanos.get()))
        }

        fn sleep(&self, duration: Duration) {
            self.sleeps.borrow_mut().push(duration);
            let slept = i64::try_from(duration.as_nanos()).expect("короткий сон") + self.sleep_oversleep_nanos;
            self.now_nanos.set(self.now_nanos.get() + slept);
        }

        fn spin_once(&self) {
            self.spins.set(self.spins.get() + 1);
            self.now_nanos.set(self.now_nanos.get() + self.spin_step_nanos);
        }
    }

    fn plan(target_nanos: i64, spin_us: u64) -> SleepUntil {
        SleepUntil {
            target: MonotonicNanos::from_nanos(target_nanos),
            spin_window: Duration::from_micros(spin_us),
            max_wait: Duration::from_millis(50),
        }
    }

    #[test]
    fn sleeps_until_the_spin_window_then_spins_to_the_target() {
        let timer = ScriptedTimer::at(0);
        // Цель через 10 мс, окно активного ожидания 500 мкс, шаг ожидания 1 мкс.
        let outcome = sleep_until(plan(10_000_000, 500), &timer);
        assert_eq!(
            outcome,
            SleepUntilOutcome::Reached {
                overshoot: Duration::ZERO
            }
        );
        assert_eq!(
            *timer.sleeps.borrow(),
            vec![Duration::from_micros(9_500)],
            "один сон до окна"
        );
        assert_eq!(timer.spins.get(), 500, "окно 500 мкс выкручено шагами по 1 мкс");
        assert_eq!(timer.now_nanos.get(), 10_000_000);
    }

    #[test]
    fn oversleeping_into_the_spin_window_is_absorbed_by_spinning() {
        let mut timer = ScriptedTimer::at(0);
        // Сон проспал на 300 мкс: из окна в 500 мкс осталось 200 мкс активного ожидания.
        timer.sleep_oversleep_nanos = 300_000;
        let outcome = sleep_until(plan(10_000_000, 500), &timer);
        assert_eq!(
            outcome,
            SleepUntilOutcome::Reached {
                overshoot: Duration::ZERO
            }
        );
        assert_eq!(timer.spins.get(), 200);
    }

    #[test]
    fn zero_spin_window_only_sleeps_and_reports_the_oversleep() {
        let mut timer = ScriptedTimer::at(0);
        timer.sleep_oversleep_nanos = 80_000;
        let outcome = sleep_until(plan(2_000_000, 0), &timer);
        assert_eq!(
            outcome,
            SleepUntilOutcome::Reached {
                overshoot: Duration::from_micros(80)
            }
        );
        assert_eq!(timer.spins.get(), 0, "без окна активного ожидания нет");
    }

    #[test]
    fn past_target_returns_immediately_with_lateness() {
        let timer = ScriptedTimer::at(5_000_000);
        let outcome = sleep_until(plan(4_000_000, 500), &timer);
        assert_eq!(
            outcome,
            SleepUntilOutcome::AlreadyPassed {
                late_by: Duration::from_millis(1)
            }
        );
        assert!(timer.sleeps.borrow().is_empty());
        assert_eq!(timer.spins.get(), 0);
    }

    #[test]
    fn target_beyond_max_wait_is_refused_without_waiting() {
        let timer = ScriptedTimer::at(0);
        let outcome = sleep_until(plan(60_000_000, 500), &timer);
        assert_eq!(
            outcome,
            SleepUntilOutcome::RefusedTooFar {
                remaining: Duration::from_millis(60)
            }
        );
        assert!(timer.sleeps.borrow().is_empty());
    }

    #[test]
    fn broken_clock_is_reported_not_waited_on() {
        let mut timer = ScriptedTimer::at(0);
        timer.fail_clock = true;
        assert_eq!(
            sleep_until(plan(1_000_000, 500), &timer),
            SleepUntilOutcome::ClockFailed(ClockReadError { errno: 22 })
        );
    }

    #[test]
    fn real_clock_reaches_a_near_target_without_waking_early() {
        let timer = super::super::monotonic_clock::SystemMonotonicTimer;
        let start = timer.now().expect("часы");
        let target = MonotonicNanos::from_nanos(start.as_nanos() + 3_000_000);
        let outcome = sleep_until(
            SleepUntil {
                target,
                spin_window: Duration::from_micros(500),
                max_wait: Duration::from_millis(50),
            },
            &timer,
        );
        assert!(matches!(outcome, SleepUntilOutcome::Reached { .. }), "{outcome:?}");
        assert!(timer.now().expect("часы") >= target, "раньше цели не просыпаемся");
    }
}
