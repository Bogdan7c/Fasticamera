//! Тесты набора сохраняемых снимков: ёмкость, срок каждого снимка, сохранение порядка.

use crate::saving::{SavingSetFull, SavingShots};
use crate::shutter::ShotRequestId;
use crate::test_support::at_ms;

#[test]
fn begin_refuses_a_full_set_without_changing_it() {
    let mut set = SavingShots::new(2);
    assert_eq!(set.begin(ShotRequestId(1), at_ms(100)), Ok(()));
    assert_eq!(set.begin(ShotRequestId(2), at_ms(200)), Ok(()));
    assert!(!set.has_room());

    assert_eq!(set.begin(ShotRequestId(3), at_ms(300)), Err(SavingSetFull));

    assert_eq!(set.len(), 2);
    assert!(set.finish(ShotRequestId(1)));
    assert!(set.has_room());
}

#[test]
fn finish_reports_absent_shot_and_expire_removes_only_overdue_ones() {
    let mut set = SavingShots::new(3);
    set.begin(ShotRequestId(1), at_ms(100)).expect("есть место");
    set.begin(ShotRequestId(2), at_ms(200)).expect("есть место");

    assert!(!set.finish(ShotRequestId(9)), "такого снимка нет");
    assert_eq!(set.expire(at_ms(99)), 0);
    assert_eq!(set.expire(at_ms(100)), 1, "срок включительно");
    assert_eq!(set.len(), 1);
    assert!(set.finish(ShotRequestId(2)), "второй остался");
    assert_eq!(set.expire(at_ms(10_000)), 0);
}

#[test]
fn zero_capacity_still_holds_one_shot() {
    let mut set = SavingShots::new(0);
    assert_eq!(set.begin(ShotRequestId(1), at_ms(1)), Ok(()));
    assert!(!set.has_room());
}
