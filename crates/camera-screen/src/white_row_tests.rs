//! Тесты мест ряда белого: порядок слева направо и вывод числа источников из него.

use super::{SOURCE_BUTTON_COUNT, WhiteRowSlot};

#[test]
fn the_row_runs_auto_then_phone_then_the_lights_in_config_order() {
    let row: Vec<WhiteRowSlot> = WhiteRowSlot::row(3).collect();
    assert_eq!(
        row,
        [
            WhiteRowSlot::Auto,
            WhiteRowSlot::Phone,
            WhiteRowSlot::Light(0),
            WhiteRowSlot::Light(1),
            WhiteRowSlot::Light(2),
        ]
    );
}

#[test]
fn positions_are_consecutive_from_zero_and_match_the_row_length() {
    let positions: Vec<usize> = WhiteRowSlot::row(5).map(WhiteRowSlot::position).collect();
    assert_eq!(positions, [0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(WhiteRowSlot::row_len(5), 7);
    assert_eq!(WhiteRowSlot::row(5).count(), WhiteRowSlot::row_len(5));
}

#[test]
fn source_buttons_are_exactly_the_ones_before_the_first_light() {
    assert_eq!(SOURCE_BUTTON_COUNT, 2);
    let sources = WhiteRowSlot::row(4)
        .filter(|slot| !matches!(slot, WhiteRowSlot::Light(_)))
        .count();
    assert_eq!(sources, SOURCE_BUTTON_COUNT);
    assert_eq!(WhiteRowSlot::row_len(0), SOURCE_BUTTON_COUNT);
}
