//! Точечные намерения колёс цвета `SetWheelPush` / `SetWheelRing` в библиотеке (14b2, решения 206, 207).

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{TempDir, environment, knob, new_library};
use look_contract::{
    ColorWheel, EditState, GlobalKnob, LookControl, LookIntent, LookSettings, WheelPart, WheelPush, WheelZone,
};
use look_presets::{LibraryChange, PRODUCT_MIGRATIONS, PresetFileStore, PresetLibrary, PresetStoreConfig};

fn push(a: f32, b: f32) -> WheelPush {
    WheelPush::new(a, b).unwrap()
}

fn wheel_of(library: &PresetLibrary, zone: WheelZone) -> ColorWheel {
    library.active_look().recipe.settings.wheels.wheel(zone)
}

#[test]
fn set_wheel_push_changes_only_that_zone_and_only_the_push() {
    let mut library = new_library();
    // Заранее трогаем кольцо Gamma, ручку и другое колесо: всё это должно пережить толчок Gamma.
    library.set_wheel_ring(WheelZone::Gamma, knob(0.5)).unwrap();
    library.set_wheel_push(WheelZone::Lift, push(0.1, 0.2)).unwrap();
    library.set_knob(GlobalKnob::Warmth, knob(0.3)).unwrap();
    let before = library.active_look().recipe;

    let change = library
        .apply(LookIntent::SetWheelPush {
            zone: WheelZone::Gamma,
            push: push(-0.4, 0.3),
        })
        .unwrap();

    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let after = library.active_look().recipe;
    assert_eq!(wheel_of(&library, WheelZone::Gamma).push, push(-0.4, 0.3));
    assert_eq!(
        wheel_of(&library, WheelZone::Gamma).ring,
        knob(0.5),
        "кольцо этого колеса цело"
    );
    for zone in [WheelZone::Lift, WheelZone::Gain, WheelZone::Offset] {
        assert_eq!(
            after.settings.wheels.wheel(zone),
            before.settings.wheels.wheel(zone),
            "{zone:?}"
        );
    }
    assert_eq!(after.settings.warmth, knob(0.3), "ручка цела");
    assert_eq!(after.base, before.base);
    assert_eq!(after.settings.bands, before.settings.bands);
}

#[test]
fn set_wheel_ring_changes_only_that_zone_and_only_the_ring() {
    let mut library = new_library();
    library.set_wheel_push(WheelZone::Gain, push(0.5, -0.2)).unwrap();
    library.set_wheel_ring(WheelZone::Offset, knob(-0.4)).unwrap();
    library.set_knob(GlobalKnob::Contrast, knob(0.25)).unwrap();
    let before = library.active_look().recipe;

    let change = library
        .apply(LookIntent::SetWheelRing {
            zone: WheelZone::Gain,
            ring: knob(0.6),
        })
        .unwrap();

    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let after = library.active_look().recipe;
    assert_eq!(wheel_of(&library, WheelZone::Gain).ring, knob(0.6));
    assert_eq!(
        wheel_of(&library, WheelZone::Gain).push,
        push(0.5, -0.2),
        "толчок этого колеса цел"
    );
    for zone in [WheelZone::Lift, WheelZone::Gamma, WheelZone::Offset] {
        assert_eq!(
            after.settings.wheels.wheel(zone),
            before.settings.wheels.wheel(zone),
            "{zone:?}"
        );
    }
    assert_eq!(after.settings.contrast, knob(0.25));
    assert_eq!(after.base, before.base);
}

#[test]
fn repeating_the_same_wheel_value_is_unchanged_and_needs_no_save() {
    let mut library = new_library();
    library.set_wheel_push(WheelZone::Lift, push(0.3, 0.3)).unwrap();
    library.set_wheel_ring(WheelZone::Lift, knob(0.2)).unwrap();
    let before = library.snapshot();

    let same_push = library.set_wheel_push(WheelZone::Lift, push(0.3, 0.3)).unwrap();
    let same_ring = library.set_wheel_ring(WheelZone::Lift, knob(0.2)).unwrap();

    assert_eq!(
        (same_push, same_ring),
        (LibraryChange::Unchanged, LibraryChange::Unchanged)
    );
    assert!(!same_push.needs_save() && !same_ring.needs_save() && !same_push.look_changed());
    assert_eq!(library.snapshot(), before, "состояние не изменилось");
}

#[test]
fn neutral_wheel_value_on_a_clean_library_is_unchanged() {
    let mut library = new_library();
    let before = library.snapshot();
    assert_eq!(
        library.set_wheel_push(WheelZone::Gain, WheelPush::NEUTRAL).unwrap(),
        LibraryChange::Unchanged
    );
    assert_eq!(
        library.set_wheel_ring(WheelZone::Gain, knob(0.0)).unwrap(),
        LibraryChange::Unchanged
    );
    assert_eq!(library.snapshot(), before);
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn push_then_reset_of_its_part_returns_to_clean_and_the_part_no_longer_differs() {
    let mut library = new_library();
    library.set_wheel_push(WheelZone::Gamma, push(0.6, 0.1)).unwrap();
    assert_eq!(library.active_look().edits, EditState::EditedOnTop);
    let control = LookControl::Wheel {
        zone: WheelZone::Gamma,
        part: WheelPart::Push,
    };
    let active = library.active_look();
    assert!(active.recipe.differs_from(&active.preset_recipe, &control));

    let change = library.apply(LookIntent::ResetControl(control.clone())).unwrap();

    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let active = library.active_look();
    assert_eq!(active.edits, EditState::Clean);
    assert!(!active.recipe.differs_from(&active.preset_recipe, &control));
    assert_eq!(wheel_of(&library, WheelZone::Gamma).push, WheelPush::NEUTRAL);
}

#[test]
fn push_and_ring_in_two_consecutive_intents_do_not_overwrite_each_other() {
    // Урок двух пальцев: толчок и кольцо одного колеса приходят отдельными намерениями из одного кадра.
    let mut library = new_library();
    library
        .apply(LookIntent::SetWheelPush {
            zone: WheelZone::Lift,
            push: push(0.2, -0.5),
        })
        .unwrap();
    library
        .apply(LookIntent::SetWheelRing {
            zone: WheelZone::Lift,
            ring: knob(0.7),
        })
        .unwrap();
    assert_eq!(
        wheel_of(&library, WheelZone::Lift),
        ColorWheel {
            push: push(0.2, -0.5),
            ring: knob(0.7)
        }
    );
    // И в обратном порядке на другом колесе.
    library.set_wheel_ring(WheelZone::Gain, knob(-0.3)).unwrap();
    library.set_wheel_push(WheelZone::Gain, push(-0.1, 0.9)).unwrap();
    assert_eq!(
        wheel_of(&library, WheelZone::Gain),
        ColorWheel {
            push: push(-0.1, 0.9),
            ring: knob(-0.3)
        }
    );
}

#[test]
fn wheel_edits_are_written_to_the_presets_file_and_survive_reload() {
    let dir = TempDir::new("wheels-reload");
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
        .set_wheel_push(WheelZone::Offset, push(0.0, -0.8))
        .unwrap();
    first.library.set_wheel_ring(WheelZone::Offset, knob(0.35)).unwrap();
    first.store.save(&first.library.snapshot()).unwrap();

    let second = open();

    assert_eq!(second.library.active_look(), first.library.active_look());
    let reloaded = second
        .library
        .active_look()
        .recipe
        .settings
        .wheels
        .wheel(WheelZone::Offset);
    assert_eq!(
        reloaded,
        ColorWheel {
            push: push(0.0, -0.8),
            ring: knob(0.35)
        }
    );
    assert_ne!(second.library.active_look().recipe.settings, LookSettings::NEUTRAL);
}
