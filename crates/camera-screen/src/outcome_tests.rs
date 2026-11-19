//! Тесты счётчиков отказов: полнота таблицы причин и независимость ячеек.

use crate::outcome::{Rejection, RejectionCounters};

#[test]
fn every_rejection_has_its_own_counter_slot_in_the_order_of_all() {
    // `ALL` — полный список в порядке номеров ячеек: пропущенная или перепутанная причина ломает тест,
    // а новый вариант `Rejection` не скомпилируется без номера в `slot`.
    for (index, reason) in Rejection::ALL.iter().enumerate() {
        assert_eq!(reason.slot(), index, "{reason:?}");
    }
}

#[test]
fn counters_count_each_reason_separately_and_sum_up() {
    let mut counters = RejectionCounters::default();
    for reason in Rejection::ALL {
        for _ in 0..=reason.slot() {
            counters.record(reason);
        }
    }
    for reason in Rejection::ALL {
        let expected = u32::try_from(reason.slot() + 1).expect("мало");
        assert_eq!(counters.get(reason), expected, "{reason:?}");
    }
    let total: usize = (1..=Rejection::ALL.len()).sum();
    assert_eq!(counters.total(), u32::try_from(total).expect("мало"));
}

#[test]
fn counters_saturate_instead_of_overflowing() {
    let mut counters = RejectionCounters::default();
    counters.counts[Rejection::ShutterBusy.slot()] = u32::MAX;
    counters.record(Rejection::ShutterBusy);
    assert_eq!(counters.get(Rejection::ShutterBusy), u32::MAX);
}
