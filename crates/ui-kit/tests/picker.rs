#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K5 «Выбор из вариантов» и K6 «Переключатель-значок».

use ui_kit::picker::{
    IconId, IconToggle, LabelId, OptionIcon, OptionPicker, PickerError, PickerOption, SelectionChange, TapBehavior,
    ToggleAction,
};

/// Значения тестовых вариантов — «режимы вспышки»; сам элемент про вспышку не знает.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flash {
    Off,
    On,
    Auto,
}

/// Разные подписи → разные иконки (набор атласа маленький, поэтому по кругу).
fn icon_for(label: u32) -> IconId {
    IconId::ALL[label as usize % IconId::ALL.len()]
}

fn option(value: Flash, label: u32) -> PickerOption<Flash> {
    PickerOption {
        value,
        label: LabelId(label),
        icon: OptionIcon::Icon(icon_for(label)),
    }
}

fn flash_options() -> Vec<PickerOption<Flash>> {
    vec![option(Flash::Off, 1), option(Flash::On, 2), option(Flash::Auto, 3)]
}

fn flash_picker() -> OptionPicker<Flash> {
    OptionPicker::new(flash_options(), Flash::Off).unwrap()
}

#[test]
fn picker_selects_by_index_and_by_value_and_reports_what_changed() {
    let mut picker = flash_picker();
    assert_eq!(picker.selected().value, Flash::Off);
    assert_eq!(
        picker.select_index(2).unwrap(),
        SelectionChange::Changed {
            from: Flash::Off,
            to: Flash::Auto
        }
    );
    assert_eq!(picker.selected().label, LabelId(3));
    assert_eq!(picker.selected().icon, OptionIcon::Icon(icon_for(3)));
    assert_eq!(picker.select_index(2).unwrap(), SelectionChange::Unchanged);
    assert_eq!(
        picker.select_value(Flash::On).unwrap(),
        SelectionChange::Changed {
            from: Flash::Auto,
            to: Flash::On
        }
    );
    assert_eq!(picker.selected_index(), 1);
    assert_eq!(picker.options().len(), 3);
}

#[test]
fn invalid_selection_is_an_explicit_error_and_keeps_the_state() {
    let mut picker = flash_picker();
    picker.select_index(1).unwrap();
    assert_eq!(
        picker.select_index(3),
        Err(PickerError::IndexOutOfRange { index: 3, length: 3 })
    );
    let short = OptionPicker::new(vec![option(Flash::Off, 1), option(Flash::On, 2)], Flash::Off).unwrap();
    let mut short = short;
    assert_eq!(short.select_value(Flash::Auto), Err(PickerError::UnknownValue));
    assert_eq!(picker.selected().value, Flash::On, "ошибка выбор не меняет");
    assert_eq!(short.selected().value, Flash::Off);
}

#[test]
fn picker_construction_rejects_empty_duplicate_and_foreign_selection() {
    assert_eq!(
        OptionPicker::<Flash>::new(vec![], Flash::Off),
        Err(PickerError::EmptyOptions)
    );
    assert_eq!(
        OptionPicker::new(vec![option(Flash::Off, 1), option(Flash::Off, 2)], Flash::Off),
        Err(PickerError::DuplicateValue)
    );
    assert_eq!(
        OptionPicker::new(vec![option(Flash::Off, 1)], Flash::On),
        Err(PickerError::UnknownValue)
    );
    // Вариант без иконки допустим.
    let label_only = PickerOption {
        value: Flash::Off,
        label: LabelId(9),
        icon: OptionIcon::LabelOnly,
    };
    let picker = OptionPicker::new(vec![label_only], Flash::Off).unwrap();
    assert_eq!(picker.options(), [label_only].as_slice());
    assert_eq!(picker.selected().label, LabelId(9));
    assert_eq!(picker.selected().icon, OptionIcon::LabelOnly);
    assert_eq!(picker.selected_index(), 0);
}

#[test]
fn icon_toggle_cycles_through_options_on_tap_and_wraps() {
    let mut toggle = IconToggle::new(flash_picker(), TapBehavior::CycleThroughOptions);
    let mut seen = Vec::new();
    for _ in 0..4 {
        match toggle.tapped().unwrap() {
            ToggleAction::Selected { to, .. } => seen.push(to),
            other => panic!("ожидали смену варианта, получили {other:?}"),
        }
    }
    assert_eq!(
        seen,
        vec![Flash::On, Flash::Auto, Flash::Off, Flash::On],
        "по кругу, с возвратом в начало"
    );
    assert_eq!(toggle.picker().selected().value, Flash::On);
}

#[test]
fn icon_toggle_with_submenu_behavior_opens_the_submenu_and_keeps_the_choice() {
    let mut toggle = IconToggle::new(flash_picker(), TapBehavior::OpenSubmenu);
    assert_eq!(toggle.tapped().unwrap(), ToggleAction::OpenSubmenu);
    assert_eq!(
        toggle.picker().selected().value,
        Flash::Off,
        "тап по значку подменю выбор не меняет"
    );
    // Выбор из подменю.
    assert_eq!(
        toggle.select_value(Flash::Auto).unwrap(),
        SelectionChange::Changed {
            from: Flash::Off,
            to: Flash::Auto
        }
    );
    assert_eq!(toggle.select_value(Flash::Auto).unwrap(), SelectionChange::Unchanged);
    assert_eq!(toggle.tap_behavior(), TapBehavior::OpenSubmenu);
}

#[test]
fn long_press_always_opens_the_submenu_without_cycling() {
    for behavior in [TapBehavior::CycleThroughOptions, TapBehavior::OpenSubmenu] {
        let toggle = IconToggle::new(flash_picker(), behavior);
        assert_eq!(toggle.long_pressed(), ToggleAction::OpenSubmenu);
        assert_eq!(toggle.picker().selected().value, Flash::Off);
    }
}

#[test]
fn toggle_with_one_option_stays_put_and_unknown_value_is_an_error() {
    let single = OptionPicker::new(vec![option(Flash::Off, 1)], Flash::Off).unwrap();
    let mut toggle = IconToggle::new(single, TapBehavior::CycleThroughOptions);
    assert_eq!(toggle.tapped().unwrap(), ToggleAction::Unchanged);
    assert_eq!(toggle.select_value(Flash::On), Err(PickerError::UnknownValue));
    assert_eq!(toggle.picker().selected().value, Flash::Off);
}
