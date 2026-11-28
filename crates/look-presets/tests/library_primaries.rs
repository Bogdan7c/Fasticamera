//! Точечное намерение полосок по каналам `SetPrimary` в библиотеке (14t, решения 339–343).

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{TempDir, environment, knob, new_library};
use look_contract::{
    EditState, GlobalKnob, LookControl, LookIntent, LookKnob, PresetError, PrimaryChannel, WheelPart, WheelPush,
    WheelZone,
};
use look_presets::{LibraryChange, PRODUCT_MIGRATIONS, PresetFileStore, PresetLibrary, PresetStoreConfig};

fn bar(library: &PresetLibrary, zone: WheelZone, channel: PrimaryChannel) -> LookKnob {
    library.active_look().recipe.settings.primaries.value(zone, channel)
}

#[test]
fn set_primary_changes_exactly_one_of_twelve_bars_and_nothing_else() {
    let mut library = new_library();
    // Заранее трогаем соседнюю полоску, колесо и ручку: всё это должно пережить правку.
    library
        .set_primary(WheelZone::Gain, PrimaryChannel::Red, knob(0.4))
        .unwrap();
    library
        .set_wheel_push(WheelZone::Gain, WheelPush::new(0.2, 0.1).unwrap())
        .unwrap();
    library.set_knob(GlobalKnob::Warmth, knob(0.3)).unwrap();
    let before = library.active_look().recipe;

    let change = library
        .apply(LookIntent::SetPrimary {
            zone: WheelZone::Gain,
            channel: PrimaryChannel::Blue,
            value: knob(-0.6),
        })
        .unwrap();

    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    assert!(change.needs_save() && change.look_changed());
    let after = library.active_look().recipe;
    for zone in WheelZone::ALL {
        for channel in PrimaryChannel::ALL {
            let expected = if (zone, channel) == (WheelZone::Gain, PrimaryChannel::Blue) {
                knob(-0.6)
            } else {
                before.settings.primaries.value(zone, channel)
            };
            assert_eq!(bar(&library, zone, channel), expected, "{zone:?}/{channel:?}");
        }
    }
    assert_eq!(after.settings.wheels, before.settings.wheels, "колёса целы");
    assert_eq!(after.settings.warmth, knob(0.3), "ручка цела");
    assert_eq!(after.base, before.base);
    assert_eq!(after.settings.bands, before.settings.bands);
}

#[test]
fn repeating_the_same_bar_value_is_unchanged_and_needs_no_save() {
    let mut library = new_library();
    library
        .set_primary(WheelZone::Lift, PrimaryChannel::Green, knob(0.25))
        .unwrap();
    let before = library.snapshot();

    let same = library
        .set_primary(WheelZone::Lift, PrimaryChannel::Green, knob(0.25))
        .unwrap();

    assert_eq!(same, LibraryChange::Unchanged);
    assert!(!same.needs_save() && !same.look_changed());
    assert_eq!(library.snapshot(), before, "состояние не изменилось");
}

#[test]
fn neutral_bar_value_on_a_clean_library_is_unchanged() {
    let mut library = new_library();
    let before = library.snapshot();
    assert_eq!(
        library
            .set_primary(WheelZone::Offset, PrimaryChannel::Red, knob(0.0))
            .unwrap(),
        LibraryChange::Unchanged
    );
    assert_eq!(library.snapshot(), before);
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn setting_a_bar_back_to_the_preset_value_returns_to_clean() {
    let mut library = new_library();
    library
        .set_primary(WheelZone::Gamma, PrimaryChannel::Blue, knob(0.5))
        .unwrap();
    assert_eq!(library.active_look().edits, EditState::EditedOnTop);

    let back = library
        .set_primary(WheelZone::Gamma, PrimaryChannel::Blue, knob(0.0))
        .unwrap();

    assert_eq!(back, LibraryChange::VisibleLookAndStoredState);
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn reset_control_returns_only_that_bar_to_the_preset_value() {
    let mut library = new_library();
    library
        .set_primary(WheelZone::Lift, PrimaryChannel::Red, knob(0.5))
        .unwrap();
    library
        .set_primary(WheelZone::Lift, PrimaryChannel::Green, knob(-0.3))
        .unwrap();
    let control = LookControl::Primary {
        zone: WheelZone::Lift,
        channel: PrimaryChannel::Red,
    };
    let active = library.active_look();
    assert!(active.recipe.differs_from(&active.preset_recipe, &control));

    let change = library.apply(LookIntent::ResetControl(control.clone())).unwrap();

    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let active = library.active_look();
    assert!(!active.recipe.differs_from(&active.preset_recipe, &control));
    assert_eq!(bar(&library, WheelZone::Lift, PrimaryChannel::Red), knob(0.0));
    assert_eq!(
        bar(&library, WheelZone::Lift, PrimaryChannel::Green),
        knob(-0.3),
        "соседняя полоска осталась правкой"
    );
    assert_eq!(active.edits, EditState::EditedOnTop);

    // Сброс уже совпадающей полоски — no-op без записи.
    let again = library.apply(LookIntent::ResetControl(control)).unwrap();
    assert_eq!(again, LibraryChange::Unchanged);

    // Сброс последнего отличия — правок больше нет.
    let last = LookControl::Primary {
        zone: WheelZone::Lift,
        channel: PrimaryChannel::Green,
    };
    library.apply(LookIntent::ResetControl(last)).unwrap();
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn reset_goes_to_the_user_preset_value_not_to_zero() {
    let mut library = new_library();
    library
        .set_primary(WheelZone::Offset, PrimaryChannel::Blue, knob(0.4))
        .unwrap();
    library.save_as_new_preset(common::name("With bars")).unwrap();
    library
        .set_primary(WheelZone::Offset, PrimaryChannel::Blue, knob(-0.7))
        .unwrap();

    library
        .reset_control(LookControl::Primary {
            zone: WheelZone::Offset,
            channel: PrimaryChannel::Blue,
        })
        .unwrap();

    assert_eq!(bar(&library, WheelZone::Offset, PrimaryChannel::Blue), knob(0.4));
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn bars_do_not_disturb_other_controls_reset_and_failed_intents_keep_state() {
    let mut library = new_library();
    library
        .set_primary(WheelZone::Gain, PrimaryChannel::Green, knob(0.3))
        .unwrap();
    let before = library.snapshot();
    // Сброс чужого органа (колесо) не трогает полоску.
    let wheel_reset = library
        .reset_control(LookControl::Wheel {
            zone: WheelZone::Gain,
            part: WheelPart::Whole,
        })
        .unwrap();
    assert_eq!(wheel_reset, LibraryChange::Unchanged);
    // Неизвестная полоса оттенка — отказ, состояние цело.
    let unknown = library.reset_control(LookControl::Band {
        band: look_contract::HueBandId::new("nope").unwrap(),
        axis: look_contract::BandAxis::Hue,
    });
    assert!(matches!(unknown, Err(PresetError::UnknownControl(_))));
    assert_eq!(library.snapshot(), before);
}

#[test]
fn bars_are_written_to_the_presets_file_and_survive_a_restart() {
    let dir = TempDir::new("bars-reload");
    let open = || {
        PresetFileStore::open(
            dir.path(),
            &PresetStoreConfig::default(),
            &environment(),
            PRODUCT_MIGRATIONS,
        )
        .expect("конфиг по умолчанию валиден")
    };
    let mut first = open();
    first
        .library
        .set_primary(WheelZone::Lift, PrimaryChannel::Red, knob(-0.45))
        .unwrap();
    first
        .library
        .set_primary(WheelZone::Offset, PrimaryChannel::Blue, knob(0.2))
        .unwrap();
    first.store.save(&first.library.snapshot()).unwrap();

    let second = open();
    assert_eq!(second.library.snapshot(), first.library.snapshot());
    assert_eq!(bar(&second.library, WheelZone::Lift, PrimaryChannel::Red), knob(-0.45));
    assert_eq!(bar(&second.library, WheelZone::Offset, PrimaryChannel::Blue), knob(0.2));
    assert_eq!(bar(&second.library, WheelZone::Gamma, PrimaryChannel::Green), knob(0.0));
}
