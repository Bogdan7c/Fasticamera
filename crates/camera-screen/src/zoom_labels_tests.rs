use super::*;
use crate::test_support::{ratio, zoom_catalog};

#[test]
fn unselected_labels_follow_the_rule_for_buttons() {
    // Ширик 0,593 → «.6» (одна цифра после точки, без нуля и без «×»), целые — без десятичной части.
    for (value, expected) in [
        (0.593, ".6"),
        (0.6, ".6"),
        (1.0, "1"),
        (2.0, "2"),
        (1.4, "1.4"),
        (3.0, "3"),
        (0.96, "1"),
        (10.0, "10"),
        (2.04, "2"),
    ] {
        assert_eq!(unselected_label(ratio(value)), expected, "значение {value}");
    }
}

#[test]
fn selected_labels_show_the_current_value_with_the_times_sign() {
    for (value, expected) in [
        (0.593, "0.6\u{d7}"),
        (1.0, "1\u{d7}"),
        (1.4, "1.4\u{d7}"),
        (1.74, "1.7\u{d7}"),
        (2.0, "2\u{d7}"),
        (9.96, "10\u{d7}"),
        (10.0, "10\u{d7}"),
        (0.96, "1\u{d7}"),
        (10.4, "10\u{d7}"),
    ] {
        assert_eq!(selected_label(ratio(value)), expected, "значение {value}");
    }
}

#[test]
fn labels_use_only_characters_the_font_atlas_has() {
    // Символы подписей обязаны быть в запечённом наборе, иначе кадр получит `UnsupportedCharacter`.
    for value in [0.593_f32, 1.0, 1.4, 2.0, 10.0] {
        for text in [unselected_label(ratio(value)), selected_label(ratio(value))] {
            assert!(
                text.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '\u{d7}'),
                "{text}"
            );
        }
    }
}

#[test]
fn focal_labels_are_the_reference_focal_times_the_ratio() {
    let catalog = zoom_catalog();
    let group = catalog.back.as_ref().expect("задняя группа");
    // Опорное фокусное — основной камеры (24 мм): 0,593 → 14 MM, 1 → 24 MM, 2 → 48 MM.
    assert_eq!(focal_label(group, ratio(0.593)).as_deref(), Some("14MM"));
    assert_eq!(focal_label(group, ratio(1.0)).as_deref(), Some("24MM"));
    assert_eq!(focal_label(group, ratio(1.7)).as_deref(), Some("41MM"));
    assert_eq!(focal_label(group, ratio(2.0)).as_deref(), Some("48MM"));
    assert_eq!(reference_focal_mm(group), Some(24.0));
}

#[test]
fn a_group_without_lenses_has_no_focal_label() {
    let mut catalog = (*zoom_catalog()).clone();
    let group = catalog.back.as_mut().expect("задняя группа");
    group.lenses.clear();
    assert_eq!(focal_label(group, ratio(1.0)), None);
}
