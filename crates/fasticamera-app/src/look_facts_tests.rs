//! Тесты фактов о виде: каталог полос с цветами точек и наблюдатель за изменениями
//! (шлёт только изменившееся, видит правки, смену основы и новую проблему).

use look_contract::{
    BuiltinPreset, EditState, GlobalKnob, LookControl, LookIntent, LookKnob, LookProblem, LookProblemKind,
    LookSettings, PresetId, PresetName, ToneBaseModel,
};
use render_loop::config::RenderLoopConfig;

use super::*;
use crate::look_test_support::{StubPersistence, handle_with};

fn catalog() -> LookFactsCatalog {
    LookFactsCatalog::from_render_config(&RenderLoopConfig::default(), &BandSwatchStyle::default())
}

fn source() -> (LookFactsSource, LookHandle) {
    let handle = handle_with(StubPersistence::accepting());
    (LookFactsSource::new(handle.clone(), catalog()), handle)
}

fn changed(change: LookFactsChange) -> LookFacts {
    match change {
        LookFactsChange::Changed(facts) => *facts,
        LookFactsChange::Unchanged => panic!("ожидались изменившиеся факты"),
    }
}

fn contrast(value: f32) -> LookSettings {
    LookSettings::NEUTRAL.with_knob(GlobalKnob::Contrast, LookKnob::new(value).expect("ручка в диапазоне"))
}

#[test]
fn the_catalog_lists_every_band_of_the_render_model_in_order_with_its_own_swatch() {
    let config = RenderLoopConfig::default();
    let bands = LookFactsCatalog::from_render_config(&config, &BandSwatchStyle::default());
    let expected_ids: Vec<_> = config
        .viewfinder
        .look_model
        .bands
        .iter()
        .map(|band| band.id.clone())
        .collect();
    let ids: Vec<_> = bands.bands().iter().map(|band| band.id.clone()).collect();
    assert_eq!(ids, expected_ids);
    // Цвета точек различимы: две полосы с одним цветом нарисовали бы неразличимые точки.
    for (index, first) in bands.bands().iter().enumerate() {
        for second in &bands.bands()[index + 1..] {
            assert_ne!(first.swatch, second.swatch, "{:?} и {:?}", first.id, second.id);
        }
    }
}

#[test]
fn band_swatches_follow_the_hue_of_the_band_centre() {
    let catalog = catalog();
    let swatch = |name: &str| {
        catalog
            .bands()
            .iter()
            .find(|band| band.id.as_str() == name)
            .unwrap_or_else(|| panic!("полосы {name} нет в модели по умолчанию"))
            .swatch
            .srgb
    };
    let [red_r, red_g, red_b] = swatch("red");
    assert!(
        red_r > red_g && red_r > red_b,
        "красная точка красная: {:?}",
        swatch("red")
    );
    let [green_r, green_g, green_b] = swatch("green");
    assert!(
        green_g > green_r && green_g > green_b,
        "зелёная точка зелёная: {:?}",
        swatch("green")
    );
    let [blue_r, blue_g, blue_b] = swatch("blue");
    assert!(
        blue_b > blue_r && blue_b > blue_g,
        "синяя точка синяя: {:?}",
        swatch("blue")
    );
}

#[test]
fn the_first_observation_is_a_change_and_describes_the_library_as_it_is() {
    let (mut source, handle) = source();
    let facts = changed(source.observe(None));

    let active = handle.active_look();
    assert_eq!(facts.active, active.preset);
    assert_eq!(facts.current, active.recipe);
    assert_eq!(facts.preset, active.preset_recipe);
    assert_eq!(facts.edits, EditState::Clean);
    assert_eq!(facts.presets, handle.view_state().presets);
    assert!(
        facts
            .presets
            .iter()
            .any(|entry| entry.id == PresetId::Builtin(BuiltinPreset::AsEye))
    );
    assert_eq!(facts.bands, catalog().bands().to_vec());
    assert_eq!(facts.problem, None);
}

#[test]
fn an_unchanged_library_is_not_sent_twice() {
    let (mut source, _handle) = source();
    changed(source.observe(None));
    assert_eq!(source.observe(None), LookFactsChange::Unchanged);
    assert_eq!(source.observe(None), LookFactsChange::Unchanged);
}

#[test]
fn a_knob_edit_changes_the_facts_and_only_that_control_differs_from_the_preset() {
    let (mut source, handle) = source();
    changed(source.observe(None));

    handle
        .apply(LookIntent::SetLook(contrast(0.4)))
        .expect("правка принята");
    let facts = changed(source.observe(None));

    assert_eq!(facts.edits, EditState::EditedOnTop);
    assert!(facts.control_differs(&LookControl::Knob(GlobalKnob::Contrast)));
    assert!(!facts.control_differs(&LookControl::Knob(GlobalKnob::Warmth)));
    assert!(!facts.control_differs(&LookControl::ToneBase));
    assert_eq!(source.observe(None), LookFactsChange::Unchanged);
}

#[test]
fn switching_the_base_reaches_the_screen_facts_as_a_change() {
    let (mut source, handle) = source();
    handle
        .apply(LookIntent::SetToneBase(ToneBaseModel::Agx))
        .expect("основа принята");
    let on_agx = changed(source.observe(None));
    assert_eq!(on_agx.current.base, ToneBaseModel::Agx);

    handle
        .apply(LookIntent::SetToneBase(ToneBaseModel::Legacy))
        .expect("основа принята");
    let on_legacy = changed(source.observe(None));
    assert_eq!(on_legacy.current.base, ToneBaseModel::Legacy);
    assert_eq!(source.observe(None), LookFactsChange::Unchanged);
}

#[test]
fn a_new_preset_appears_in_the_list_and_becomes_active() {
    let (mut source, handle) = source();
    let before = changed(source.observe(None));
    handle
        .apply(LookIntent::SaveAsNewPreset(PresetName::new("Sunset").expect("имя")))
        .expect("пресет сохранён");

    let after = changed(source.observe(None));
    assert_eq!(after.presets.len(), before.presets.len() + 1);
    let added = after
        .presets
        .iter()
        .find(|entry| entry.name.as_str() == "Sunset")
        .expect("Sunset в списке");
    assert_eq!(after.active, added.id);
}

#[test]
fn a_problem_reaches_the_facts_and_a_second_equal_problem_is_a_new_change() {
    let (mut source, _handle) = source();
    changed(source.observe(None));
    let problem = |serial| LookProblem {
        serial,
        kind: LookProblemKind::SavesRefused,
    };

    assert_eq!(changed(source.observe(Some(problem(1)))).problem, Some(problem(1)));
    assert_eq!(source.observe(Some(problem(1))), LookFactsChange::Unchanged);
    // Тот же вид проблемы, но с новым номером: экран покажет плашку снова.
    assert_eq!(changed(source.observe(Some(problem(2)))).problem, Some(problem(2)));
}

#[test]
fn band_swatch_style_from_config_changes_the_dot_colour() {
    // Цвет точки — из конфига экрана, а не из констант модуля: другая светлота даёт другую точку.
    let config = RenderLoopConfig::default();
    let default_style = BandSwatchStyle::default();
    let darker = BandSwatchStyle {
        lightness: 0.45,
        ..default_style
    };
    let usual = LookFactsCatalog::from_render_config(&config, &default_style);
    let dark = LookFactsCatalog::from_render_config(&config, &darker);
    assert!(!usual.bands().is_empty(), "в конфиге по умолчанию есть полосы");
    for (light_band, dark_band) in usual.bands().iter().zip(dark.bands()) {
        let light_sum: u32 = light_band.swatch.srgb.iter().map(|channel| u32::from(*channel)).sum();
        let dark_sum: u32 = dark_band.swatch.srgb.iter().map(|channel| u32::from(*channel)).sum();
        assert!(
            dark_sum < light_sum,
            "полоса {:?}: тёмная точка не темнее",
            light_band.id
        );
    }
}

#[test]
fn band_swatch_style_rejects_values_outside_the_range_and_nan() {
    assert_eq!(BandSwatchStyle::default().validate(), Ok(()));
    for lightness in [0.0, -0.1, 1.01, f32::NAN] {
        let style = BandSwatchStyle {
            lightness,
            ..BandSwatchStyle::default()
        };
        assert!(
            matches!(style.validate(), Err(BandSwatchStyleError::Lightness { .. })),
            "светлота {lightness} принята"
        );
    }
    for chroma in [-0.01, MAX_SWATCH_CHROMA + 0.01, f32::NAN] {
        let style = BandSwatchStyle {
            chroma,
            ..BandSwatchStyle::default()
        };
        assert!(
            matches!(style.validate(), Err(BandSwatchStyleError::Chroma { .. })),
            "цветность {chroma} принята"
        );
    }
}

#[test]
fn quiet_frames_do_not_rebuild_the_facts_and_a_change_rebuilds_exactly_once() {
    let (mut source, handle) = source();
    changed(source.observe(None));
    assert_eq!(source.rebuilds(), 1, "первый снимок собран");

    for _ in 0..50 {
        assert_eq!(source.observe(None), LookFactsChange::Unchanged);
    }
    assert_eq!(source.rebuilds(), 1, "50 тихих кадров не пересобирали факты");

    handle
        .apply(LookIntent::SetLook(contrast(0.4)))
        .expect("правка принята");
    let facts = changed(source.observe(None));
    assert_eq!(source.rebuilds(), 2, "правка — одна пересборка");
    assert_eq!(source.observe(None), LookFactsChange::Unchanged);
    assert_eq!(source.rebuilds(), 2);

    // Содержимое — ровно то, что собирает прямая сборка (кэш ничего не исказил).
    assert_eq!(facts, source.current_facts(None));
    assert_eq!(facts.current.settings, contrast(0.4));
}

#[test]
fn a_new_problem_rebuilds_the_facts_even_when_the_library_is_quiet() {
    let (mut source, _handle) = source();
    changed(source.observe(None));
    let first = LookProblem {
        serial: 1,
        kind: LookProblemKind::SavesRefused,
    };
    assert_eq!(
        changed(source.observe(Some(first.clone()))).problem,
        Some(first.clone())
    );
    assert_eq!(source.rebuilds(), 2);
    assert_eq!(source.observe(Some(first)), LookFactsChange::Unchanged);
    assert_eq!(source.rebuilds(), 2, "та же проблема — без пересборки");
}

#[test]
fn facts_carry_the_wheel_ring_from_the_catalog() {
    let ring_style = crate::look_wheel_ring::WheelRingStyle {
        stops: 36,
        ..Default::default()
    };
    let handle = handle_with(StubPersistence::accepting());
    let source = LookFactsSource::new(handle, catalog().with_wheel_ring_style(&ring_style));
    let facts = source.current_facts(None);
    assert_eq!(facts.wheel_ring.stops().len(), 36);
    assert_eq!(&facts.wheel_ring, source.catalog.wheel_ring());
}
