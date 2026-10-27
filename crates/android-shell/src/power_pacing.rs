//! Темп опроса батареи: следующий момент чтения по сетке периода.
//!
//! Чтение батареи — binder-вызов, его длительность плавает. Если ждать «период после
//! чтения», реальный период расползается на длительность вызова. Поэтому моменты чтения
//! стоят на сетке `старт + k·период`; если поток опоздал больше чем на период (телефон
//! заморожен, binder завис), пропущенные узлы не догоняются пачкой — берётся ближайший
//! будущий узел, а число пропущенных возвращается для учёта.

use std::time::{Duration, Instant};

/// Следующий узел сетки и сколько узлов пропущено из-за опоздания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextRead {
    pub at: Instant,
    pub skipped_ticks: u64,
}

/// Узел сетки после `previous`, ближайший к `now` из будущих (или `previous + period`, если
/// поток не опоздал). `period` не нулевой — гарантирует `PowerSamplingPeriod`.
pub fn next_read(previous: Instant, period: Duration, now: Instant) -> NextRead {
    let candidate = previous + period;
    if candidate > now || period.is_zero() {
        return NextRead {
            at: candidate,
            skipped_ticks: 0,
        };
    }
    // Сколько целых периодов прошло после `candidate` (он сам уже в прошлом и тоже пропущен).
    let late = now.duration_since(candidate);
    let late_periods = late.as_nanos() / period.as_nanos();
    let skipped = late_periods + 1;
    let advance = period.saturating_mul(u32::try_from(skipped).unwrap_or(u32::MAX));
    NextRead {
        at: candidate + advance,
        skipped_ticks: u64::try_from(skipped).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_millis(500);

    #[test]
    fn on_time_thread_keeps_the_grid() {
        let start = Instant::now();
        let now = start + Duration::from_millis(120);
        assert_eq!(
            next_read(start, PERIOD, now),
            NextRead {
                at: start + PERIOD,
                skipped_ticks: 0
            }
        );
    }

    #[test]
    fn slow_read_does_not_shift_the_grid() {
        let start = Instant::now();
        // Чтение заняло 480 мс из 500: следующий узел всё равно через 500 мс от старта.
        let now = start + Duration::from_millis(480);
        assert_eq!(next_read(start, PERIOD, now).at, start + PERIOD);
    }

    #[test]
    fn late_thread_skips_missed_ticks_and_counts_them() {
        let start = Instant::now();
        // Опоздали на 1,2 с: узлы 0,5 и 1,0 с пропущены, следующий — 1,5 с.
        let now = start + Duration::from_millis(1_200);
        assert_eq!(
            next_read(start, PERIOD, now),
            NextRead {
                at: start + Duration::from_millis(1_500),
                skipped_ticks: 2
            }
        );
    }

    #[test]
    fn exactly_on_tick_moves_to_the_next_one() {
        let start = Instant::now();
        let now = start + PERIOD;
        assert_eq!(
            next_read(start, PERIOD, now),
            NextRead {
                at: start + 2 * PERIOD,
                skipped_ticks: 1
            }
        );
    }
}
