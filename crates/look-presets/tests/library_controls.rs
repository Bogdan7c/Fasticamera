//! Тесты рецепта в библиотеке: смена основы, сброс одной ручки к значению ПРАВЛЕННОГО пресета (решения 195, 196).

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{contrast, knob, name, new_library, with_sky_band};
use look_contract::{
    BandAxis, BuiltinPreset, ColorWheel, EditState, GlobalKnob, HueBandId, LookControl, LookIntent, LookRecipe,
    LookSettings, PresetError, PresetId, ToneBaseModel, WheelPart, WheelPush, WheelZone,
};
use look_presets::{LibraryChange, PresetLibrary};

const AS_EYE: PresetId = PresetId::Builtin(BuiltinPreset::AsEye);

/// Библиотека, где активен свой пресет с разными значениями на каждом органе: основа Filmic, контраст, полоса sky,
/// колесо Lift. Возвращает библиотеку и рецепт пресета.
fn library_with_graded_preset() -> (PresetLibrary, LookRecipe) {
    let mut library = new_library();
    library.set_tone_base(ToneBaseModel::Filmic).unwrap();
    let settings = LookSettings {
        warmth: knob(0.3),
        wheels: LookSettings::NEUTRAL.wheels.with_wheel(
            WheelZone::Lift,
            ColorWheel {
                push: WheelPush::new(0.2, 0.1).unwrap(),
                ring: knob(0.25),
            },
        ),
        ..LookSettings {
            bands: with_sky_band(0.4).bands,
            ..contrast(0.2)
        }
    };
    library.set_look(settings).unwrap();
    library.save_as_new_preset(name("Graded")).unwrap();
    let preset = library.active_look().preset_recipe;
    assert_eq!(preset.base, ToneBaseModel::Filmic);
    (library, preset)
}

fn sky(axis: BandAxis) -> LookControl {
    LookControl::Band {
        band: HueBandId::new("sky").unwrap(),
        axis,
    }
}

#[test]
fn set_tone_base_on_builtin_is_an_edit_with_settings_untouched() {
    let mut library = new_library();
    let before = library.active_look();
    assert_eq!(
        before.recipe.base,
        ToneBaseModel::Agx,
        "основа встроенных — из окружения каталога"
    );

    let change = library.apply(LookIntent::SetToneBase(ToneBaseModel::Legacy)).unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let after = library.active_look();
    assert_eq!(after.edits, EditState::EditedOnTop);
    assert_eq!(after.preset, AS_EYE);
    assert_eq!(after.recipe.base, ToneBaseModel::Legacy);
    assert_eq!(after.recipe.settings, before.recipe.settings, "ручки не тронуты");
    assert_eq!(after.preset_recipe, before.preset_recipe, "сам пресет не меняется");
}

#[test]
fn set_tone_base_back_to_preset_base_clears_edits() {
    let mut library = new_library();
    library.set_tone_base(ToneBaseModel::Aces).unwrap();
    let change = library.set_tone_base(ToneBaseModel::Agx).unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    assert_eq!(library.active_look().edits, EditState::Clean);
    // Повтор того же — ничего не меняет.
    assert_eq!(
        library.set_tone_base(ToneBaseModel::Agx).unwrap(),
        LibraryChange::Unchanged
    );
}

#[test]
fn set_look_keeps_the_current_base() {
    let mut library = new_library();
    library.set_tone_base(ToneBaseModel::Aces).unwrap();
    library.apply(LookIntent::SetLook(contrast(0.5))).unwrap();
    let active = library.active_look();
    assert_eq!(active.recipe.base, ToneBaseModel::Aces);
    assert_eq!(active.recipe.settings, contrast(0.5));
}

#[test]
fn save_as_new_and_overwrite_store_the_full_recipe_with_base() {
    let mut library = new_library();
    library.set_tone_base(ToneBaseModel::Aces).unwrap();
    library.set_look(contrast(0.4)).unwrap();
    library.save_as_new_preset(name("Mine")).unwrap();
    let saved = library.snapshot().user_presets[0].recipe.clone();
    assert_eq!(saved.base, ToneBaseModel::Aces);
    assert_eq!(saved.settings, contrast(0.4));
    assert_eq!(library.active_look().edits, EditState::Clean);

    library.set_tone_base(ToneBaseModel::Legacy).unwrap();
    let id = match library.active_look().preset {
        PresetId::User(id) => id,
        other => panic!("активен не свой пресет: {other:?}"),
    };
    library.apply(LookIntent::OverwritePreset(id)).unwrap();
    assert_eq!(library.snapshot().user_presets[0].recipe.base, ToneBaseModel::Legacy);
    assert_eq!(library.snapshot().user_presets[0].recipe.settings, contrast(0.4));
}

#[test]
fn reset_returns_each_control_kind_to_the_preset_value_and_keeps_neighbours() {
    let (mut library, preset) = library_with_graded_preset();
    // Правим всё сразу.
    let edited_settings = LookSettings {
        contrast: knob(-0.6),
        warmth: knob(-0.7),
        bands: with_sky_band(-0.9).bands,
        wheels: LookSettings::NEUTRAL.wheels.with_wheel(
            WheelZone::Lift,
            ColorWheel {
                push: WheelPush::new(-0.5, 0.0).unwrap(),
                ring: knob(-0.5),
            },
        ),
        ..LookSettings::NEUTRAL
    };
    library.set_look(edited_settings).unwrap();
    library.set_tone_base(ToneBaseModel::Legacy).unwrap();
    let edited = library.active_look().recipe;

    let cases: Vec<(LookControl, LookRecipe)> = vec![
        (
            LookControl::ToneBase,
            LookRecipe {
                base: preset.base,
                ..edited.clone()
            },
        ),
        (
            LookControl::Knob(GlobalKnob::Contrast),
            LookRecipe {
                settings: edited
                    .settings
                    .clone()
                    .with_knob(GlobalKnob::Contrast, preset.settings.contrast),
                ..edited.clone()
            },
        ),
        (
            sky(BandAxis::Hue),
            LookRecipe {
                settings: LookSettings {
                    bands: preset.settings.bands.clone(),
                    ..edited.settings.clone()
                },
                ..edited.clone()
            },
        ),
        (
            LookControl::Wheel {
                zone: WheelZone::Lift,
                part: WheelPart::Whole,
            },
            LookRecipe {
                settings: LookSettings {
                    wheels: preset.settings.wheels,
                    ..edited.settings.clone()
                },
                ..edited.clone()
            },
        ),
    ];
    for (control, expected) in cases {
        let mut trial = library.clone();
        let change = trial.apply(LookIntent::ResetControl(control.clone())).unwrap();
        assert_eq!(change, LibraryChange::VisibleLookAndStoredState, "{control:?}");
        let active = trial.active_look();
        assert_eq!(active.recipe, expected, "{control:?}");
        assert_eq!(
            active.edits,
            EditState::EditedOnTop,
            "{control:?}: остальные правки живы"
        );
        assert_eq!(active.preset_recipe, preset, "{control:?}: пресет не менялся");
    }
}

#[test]
fn reset_of_a_wheel_part_resets_only_that_part() {
    let (mut library, preset) = library_with_graded_preset();
    let lift = |push: WheelPush, ring| ColorWheel { push, ring: knob(ring) };
    let edited = LookSettings {
        wheels: preset
            .settings
            .wheels
            .with_wheel(WheelZone::Lift, lift(WheelPush::new(-0.5, 0.0).unwrap(), -0.5)),
        ..preset.settings.clone()
    };
    library.set_look(edited).unwrap();

    library
        .reset_control(LookControl::Wheel {
            zone: WheelZone::Lift,
            part: WheelPart::Ring,
        })
        .unwrap();
    let wheel = library.active_look().recipe.settings.wheels.wheel(WheelZone::Lift);
    assert_eq!(wheel.ring, knob(0.25), "кольцо вернулось к пресету");
    assert_eq!(
        wheel.push,
        WheelPush::new(-0.5, 0.0).unwrap(),
        "толчок остался правленым"
    );
}

#[test]
fn reset_of_the_last_differing_control_makes_the_look_clean() {
    let (mut library, preset) = library_with_graded_preset();
    library
        .set_look(preset.settings.clone().with_knob(GlobalKnob::Tint, knob(0.5)))
        .unwrap();
    assert_eq!(library.active_look().edits, EditState::EditedOnTop);

    let change = library.reset_control(LookControl::Knob(GlobalKnob::Tint)).unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let active = library.active_look();
    assert_eq!(active.edits, EditState::Clean);
    assert_eq!(active.recipe, preset);
    assert_eq!(library.snapshot().edits, None, "в файл правок не попадает");
}

#[test]
fn reset_of_control_already_equal_to_preset_is_unchanged_and_needs_no_save() {
    let (mut library, _) = library_with_graded_preset();
    library.set_look(contrast(-0.1)).unwrap();
    let before = library.snapshot();
    // Tint в правках и в пресете оба нулевые: сбрасывать нечего.
    let change = library.reset_control(LookControl::Knob(GlobalKnob::Tint)).unwrap();
    assert_eq!(change, LibraryChange::Unchanged);
    assert!(!change.needs_save());
    assert_eq!(library.snapshot(), before);

    // Чистый вид и орган, которого нет в правках, — тоже Unchanged.
    let mut clean = new_library();
    assert_eq!(
        clean.reset_control(LookControl::ToneBase).unwrap(),
        LibraryChange::Unchanged
    );
    assert_eq!(
        clean.reset_control(sky(BandAxis::Luminance)).unwrap(),
        LibraryChange::Unchanged
    );
}

#[test]
fn reset_of_unknown_band_is_an_error_and_changes_nothing() {
    let (mut library, _) = library_with_graded_preset();
    library.set_look(contrast(-0.1)).unwrap();
    let before = library.snapshot();
    let control = LookControl::Band {
        band: HueBandId::new("ghost").unwrap(),
        axis: BandAxis::Hue,
    };
    assert_eq!(
        library.apply(LookIntent::ResetControl(control.clone())),
        Err(PresetError::UnknownControl(control))
    );
    assert_eq!(library.snapshot(), before);
}

#[test]
fn reset_on_builtin_resets_to_the_builtin_values() {
    let mut library = new_library();
    let builtin = library.active_look().preset_recipe;
    let edited = builtin.settings.clone().with_knob(GlobalKnob::Contrast, knob(-0.9));
    library.set_look(edited).unwrap();
    library.reset_control(LookControl::Knob(GlobalKnob::Contrast)).unwrap();
    assert_eq!(library.active_look().recipe, builtin);
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn select_and_delete_semantics_survive_recipes() {
    let (mut library, preset) = library_with_graded_preset();
    library.set_tone_base(ToneBaseModel::Legacy).unwrap();
    // Выбор того же пресета снимает правки основы тоже.
    let active = library.active_look().preset;
    library.apply(LookIntent::SelectPreset(active)).unwrap();
    assert_eq!(library.active_look().recipe, preset);
    // Удаление активного возвращает стартовый встроенный с основой из окружения.
    let PresetId::User(id) = active else {
        panic!("ждали свой пресет")
    };
    library.apply(LookIntent::DeletePreset(id)).unwrap();
    let after = library.active_look();
    assert_eq!(after.preset, AS_EYE);
    assert_eq!(after.recipe.base, ToneBaseModel::Agx);
    assert_eq!(after.edits, EditState::Clean);
}

// ---- Точечные намерения SetKnob / SetBandAxis (ревью 14b) ----

#[test]
fn two_consecutive_set_knob_on_different_knobs_both_survive() {
    // Баг, ради которого введены намерения: два полных SetLook из одних устаревших фактов теряли первую правку.
    let mut library = new_library();
    library
        .apply(LookIntent::SetKnob {
            knob: GlobalKnob::Contrast,
            value: knob(0.4),
        })
        .unwrap();
    library
        .apply(LookIntent::SetKnob {
            knob: GlobalKnob::Warmth,
            value: knob(-0.3),
        })
        .unwrap();
    let active = library.active_look();
    assert_eq!(active.edits, EditState::EditedOnTop);
    assert_eq!(active.recipe.settings.contrast, knob(0.4), "первая правка не потеряна");
    assert_eq!(active.recipe.settings.warmth, knob(-0.3));
}

#[test]
fn set_knob_keeps_base_and_other_controls_and_reports_visible_change() {
    let mut library = new_library();
    library.set_tone_base(ToneBaseModel::Aces).unwrap();
    library.set_look(with_sky_band(0.5)).unwrap();
    let change = library.set_knob(GlobalKnob::Tint, knob(0.2)).unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let recipe = library.active_look().recipe;
    assert_eq!(recipe.base, ToneBaseModel::Aces);
    assert_eq!(recipe.settings.bands, with_sky_band(0.5).bands);
    assert_eq!(recipe.settings.tint, knob(0.2));
}

#[test]
fn set_knob_equal_to_current_is_unchanged_and_last_edit_back_to_preset_is_clean() {
    let mut library = new_library();
    // Значение пресета (у встроенных контраст не обязан быть нулём).
    let preset_contrast = library.active_look().preset_recipe.settings.contrast;
    assert_eq!(
        library.set_knob(GlobalKnob::Contrast, preset_contrast).unwrap(),
        LibraryChange::Unchanged
    );
    assert_eq!(library.active_look().edits, EditState::Clean);
    library.set_knob(GlobalKnob::Contrast, knob(0.7)).unwrap();
    assert_eq!(library.active_look().edits, EditState::EditedOnTop);
    library.set_knob(GlobalKnob::Contrast, preset_contrast).unwrap();
    assert_eq!(
        library.active_look().edits,
        EditState::Clean,
        "последняя правка вернулась к пресету"
    );
}

#[test]
fn set_band_axis_changes_only_that_axis_and_keeps_other_axes_and_knobs() {
    let mut library = new_library();
    let sky_id = HueBandId::new("sky").unwrap();
    library.set_knob(GlobalKnob::Contrast, knob(0.3)).unwrap();
    library
        .apply(LookIntent::SetBandAxis {
            band: sky_id.clone(),
            axis: BandAxis::Hue,
            value: knob(0.4),
        })
        .unwrap();
    let change = library
        .apply(LookIntent::SetBandAxis {
            band: sky_id.clone(),
            axis: BandAxis::Saturation,
            value: knob(-0.2),
        })
        .unwrap();
    assert_eq!(change, LibraryChange::VisibleLookAndStoredState);
    let settings = library.active_look().recipe.settings;
    let band = settings.bands.for_band(&sky_id);
    assert_eq!(band.hue, knob(0.4), "первая ось не потеряна");
    assert_eq!(band.saturation, knob(-0.2));
    assert_eq!(band.luminance, knob(0.0));
    assert_eq!(settings.contrast, knob(0.3), "глобальная ручка не тронута");
    // Повтор того же значения и возврат к нулю.
    assert_eq!(
        library
            .set_band_axis(sky_id.clone(), BandAxis::Saturation, knob(-0.2))
            .unwrap(),
        LibraryChange::Unchanged
    );
    library.set_band_axis(sky_id.clone(), BandAxis::Hue, knob(0.0)).unwrap();
    library.set_band_axis(sky_id, BandAxis::Saturation, knob(0.0)).unwrap();
    let preset_contrast = library.active_look().preset_recipe.settings.contrast;
    library.set_knob(GlobalKnob::Contrast, preset_contrast).unwrap();
    assert_eq!(library.active_look().edits, EditState::Clean);
}

#[test]
fn set_band_axis_on_unknown_band_is_refused_and_changes_nothing() {
    let mut library = new_library();
    library.set_knob(GlobalKnob::Contrast, knob(0.3)).unwrap();
    let before = library.snapshot();
    let ghost = HueBandId::new("no-such-band").unwrap();
    let result = library.apply(LookIntent::SetBandAxis {
        band: ghost.clone(),
        axis: BandAxis::Hue,
        value: knob(0.5),
    });
    assert_eq!(
        result,
        Err(PresetError::UnknownControl(LookControl::Band {
            band: ghost,
            axis: BandAxis::Hue
        }))
    );
    assert_eq!(library.snapshot(), before);
}
