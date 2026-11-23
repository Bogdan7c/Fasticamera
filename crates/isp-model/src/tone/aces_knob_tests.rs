//! Ручки тона ACES (контраст, средние тона, тени, света) и проверка настроек: каждая ручка меняет свою часть кривой
//! и не трогает остальные, нейтраль — бит в бит, края диапазонов слоя вида × подтяжка AE.

use super::AcesBase;
use super::settings::{AcesSettings, InvalidAces};
use super::tests::{base_for, default_base, grey_at, grey_pixel, tone_curve};
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;
use crate::tone::knobs::{ToneKnob, ToneKnobAmounts, ToneModelKind, ToneModelSettings, apply_knobs_lenient};

// ---- Ручки тона ----

fn knobbed(amounts: ToneKnobAmounts) -> AcesSettings {
    let outcome = apply_knobs_lenient(&AcesSettings::default(), &amounts);
    assert!(
        outcome.unsupported.is_empty(),
        "ACES поддерживает все ручки: {:?}",
        outcome.unsupported
    );
    outcome.settings
}

fn grey_output_at(settings: &AcesSettings, e: f32) -> f32 {
    base_for(settings).map_scene_pixel(grey_pixel(grey_at(settings, e)), 1.0)[0]
}

/// Наклон выхода по оси экспозиции вокруг `e` (центральная разность по `±step` ступени).
fn slope_at(settings: &AcesSettings, e: f32, step: f32) -> f32 {
    (grey_output_at(settings, e + step) - grey_output_at(settings, e - step)) / (2.0 * step)
}

#[test]
fn neutral_knobs_keep_the_settings_and_the_output_bit_for_bit() {
    let defaults = AcesSettings::default();
    let neutral = knobbed(ToneKnobAmounts::default());
    assert_eq!(neutral, defaults);
    let (plain, same) = (base_for(&defaults), base_for(&neutral));
    assert_eq!(plain.parameters(), same.parameters());
    for e in [-6.0_f32, -1.0, 0.0, 1.0, 2.5, 3.7] {
        let pixel = [
            grey_at(&defaults, e) * 1.2,
            grey_at(&defaults, e),
            grey_at(&defaults, e) * 0.7,
        ];
        assert_eq!(plain.map_scene_pixel(pixel, 1.0), same.map_scene_pixel(pixel, 1.0));
    }
}

#[test]
fn contrast_knob_scales_the_slope_around_the_grey_and_keeps_the_grey() {
    let settings = knobbed(ToneKnobAmounts {
        contrast_log2: 0.3,
        ..ToneKnobAmounts::default()
    });
    let defaults = AcesSettings::default();
    assert!((grey_output_at(&settings, 0.0) - defaults.display_middle_gray).abs() < 3e-6);
    let ratio = slope_at(&settings, 0.0, 0.02) / slope_at(&defaults, 0.0, 0.02);
    assert!(
        (ratio - 0.3_f32.exp2()).abs() < 0.02,
        "наклон вырос в {ratio}, ждали {}",
        0.3_f32.exp2()
    );
    // Контраст вниз — наклон меньше.
    let flatter = knobbed(ToneKnobAmounts {
        contrast_log2: -0.3,
        ..ToneKnobAmounts::default()
    });
    assert!(slope_at(&flatter, 0.0, 0.02) < slope_at(&defaults, 0.0, 0.02));
}

#[test]
fn midtones_knob_moves_the_display_grey_exactly() {
    for amount in [0.3_f32, -0.3] {
        let settings = knobbed(ToneKnobAmounts {
            midtones_ev: amount,
            ..ToneKnobAmounts::default()
        });
        let expected = AcesSettings::default().display_middle_gray * amount.exp2();
        assert!(
            (grey_output_at(&settings, 0.0) - expected).abs() < 3e-6,
            "{amount}: {}",
            grey_output_at(&settings, 0.0)
        );
    }
}

#[test]
fn shadows_knob_lifts_only_the_shadows() {
    let defaults = AcesSettings::default();
    let lifted = knobbed(ToneKnobAmounts {
        shadows: 0.6,
        ..ToneKnobAmounts::default()
    });
    let darkened = knobbed(ToneKnobAmounts {
        shadows: -0.6,
        ..ToneKnobAmounts::default()
    });
    assert!(
        grey_output_at(&lifted, -5.0) > grey_output_at(&defaults, -5.0) * 1.15,
        "тени не стали светлее"
    );
    assert!(
        grey_output_at(&darkened, -5.0) < grey_output_at(&defaults, -5.0) * 0.9,
        "тени не стали темнее"
    );
    // Серый и наклон на нём не меняются (C1 в нуле формы).
    assert!((grey_output_at(&lifted, 0.0) - defaults.display_middle_gray).abs() < 3e-6);
    let ratio = slope_at(&lifted, 0.0, 0.02) / slope_at(&defaults, 0.0, 0.02);
    assert!((ratio - 1.0).abs() < 0.01, "наклон на сером изменился: {ratio}");
    // Всё выше серого — бит в бит прежнее.
    for e in [0.0_f32, 0.5, 2.0, 3.5] {
        assert_eq!(grey_output_at(&lifted, e), grey_output_at(&defaults, e), "e = {e}");
    }
}

#[test]
fn highlights_knob_lifts_only_the_highlights() {
    let defaults = AcesSettings::default();
    let lifted = knobbed(ToneKnobAmounts {
        highlights: 0.6,
        ..ToneKnobAmounts::default()
    });
    let darkened = knobbed(ToneKnobAmounts {
        highlights: -0.6,
        ..ToneKnobAmounts::default()
    });
    assert!(
        grey_output_at(&lifted, 1.5) > grey_output_at(&defaults, 1.5) + 0.01,
        "света не стали светлее"
    );
    assert!(
        grey_output_at(&darkened, 1.5) < grey_output_at(&defaults, 1.5) - 0.01,
        "света не стали темнее"
    );
    assert!((grey_output_at(&lifted, 0.0) - defaults.display_middle_gray).abs() < 3e-6);
    for e in [-6.0_f32, -2.0, -0.5] {
        assert_eq!(grey_output_at(&lifted, e), grey_output_at(&defaults, e), "e = {e}");
    }
}

#[test]
fn every_knob_corner_at_the_look_edges_resolves_and_keeps_white_exact_and_curve_monotone() {
    // Диапазоны слоя вида по умолчанию: контраст ±0,1, средние тона ±0,3, тени и света ±0,6; подтяжка AE ±0,5 ступени.
    for contrast in [-0.1_f32, 0.0, 0.1] {
        for midtones in [-0.3_f32, 0.0, 0.3] {
            for shadows in [-0.6_f32, 0.0, 0.6] {
                for highlights in [-0.6_f32, 0.0, 0.6] {
                    for gain in [0.5_f32.exp2().recip(), 1.0, 0.5_f32.exp2()] {
                        let settings = knobbed(ToneKnobAmounts {
                            contrast_log2: contrast,
                            midtones_ev: midtones,
                            shadows,
                            highlights,
                        })
                        .with_exposure_gain(ExposureGain::new(gain).unwrap());
                        let label = format!("{contrast}/{midtones}/{shadows}/{highlights} k={gain}");
                        let base = AcesBase::resolve(&settings, DisplayGamut::Srgb)
                            .unwrap_or_else(|error| panic!("{label}: {error}"));
                        for white in [1.0_f32, 4.0] {
                            let output = base.map_scene_pixel(grey_pixel(white), white);
                            assert!(
                                output.iter().all(|c| (c - 1.0).abs() < 3e-6),
                                "{label}: белый {white}: {output:?}"
                            );
                        }
                        let curve = tone_curve(&base, 1.0);
                        let mut previous = curve(-9.0);
                        for step in -89..=90 {
                            let value = curve(step as f32 / 10.0);
                            assert!(
                                value >= previous - 1e-7,
                                "{label}: кривая убывает на e = {}",
                                step as f32 / 10.0
                            );
                            previous = value;
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn aces_supports_every_knob_and_names_itself() {
    let defaults = AcesSettings::default();
    for knob in [
        ToneKnob::Contrast,
        ToneKnob::Midtones,
        ToneKnob::Shadows,
        ToneKnob::Highlights,
    ] {
        let adjusted = defaults.with_knob(knob, 0.25).unwrap();
        assert_ne!(adjusted, defaults, "{knob:?} ничего не изменила");
    }
    assert_eq!(<AcesSettings as ToneModelSettings>::KIND, ToneModelKind::Aces);
}

// ---- Проверка настроек ----

#[test]
fn invalid_settings_are_rejected_with_distinct_errors_naming_the_field() {
    let resolve = |settings: AcesSettings| AcesBase::resolve(&settings, DisplayGamut::Srgb).unwrap_err();
    let defaults = AcesSettings::default;
    assert!(matches!(
        resolve(AcesSettings {
            scene_middle_gray: 0.0,
            ..defaults()
        }),
        InvalidAces::OutOfUnitInterval {
            field: "scene_middle_gray",
            ..
        }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            display_middle_gray: 1.0,
            ..defaults()
        }),
        InvalidAces::OutOfUnitInterval {
            field: "display_middle_gray",
            ..
        }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            contrast: 0.0,
            ..defaults()
        }),
        InvalidAces::NotPositive { field: "contrast", .. }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            shadows_ev: f32::NAN,
            ..defaults()
        }),
        InvalidAces::NotFinite {
            field: "shadows_ev",
            ..
        }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            glow_gain: 2.0,
            ..defaults()
        }),
        InvalidAces::OutOfRange { field: "glow_gain", .. }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            surround_gamma: 3.0,
            ..defaults()
        }),
        InvalidAces::OutOfRange {
            field: "surround_gamma",
            ..
        }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            shadows_ev: 5.0,
            ..defaults()
        }),
        InvalidAces::ShapeNotMonotone { .. }
    ));
    assert!(matches!(
        resolve(AcesSettings {
            knee_ev: 6.0,
            ..defaults()
        }),
        InvalidAces::KneeBeyondToneScale { .. }
    ));
    // Колено на 3,6 ступени при белом точки на 3,74: запаса меньше 0,25 ступени.
    assert!(matches!(
        resolve(AcesSettings {
            knee_ev: 3.6,
            ..defaults()
        }),
        InvalidAces::WhiteTooCloseToKnee { .. }
    ));
    let message = resolve(AcesSettings {
        knee_ev: 6.0,
        ..defaults()
    })
    .to_string();
    assert!(message.contains("колено") && message.contains('6'), "{message}");
}

#[test]
fn settings_round_trip_through_json_and_reject_unknown_fields() {
    let defaults = AcesSettings::default();
    let text = serde_json::to_string(&defaults).unwrap();
    assert_eq!(serde_json::from_str::<AcesSettings>(&text).unwrap(), defaults);
    // Частичная настройка: остальное — умолчания.
    let partial: AcesSettings = serde_json::from_str(r#"{"knee_ev": 1.2, "contrast": 1.1}"#).unwrap();
    assert_eq!(partial.knee_ev, 1.2);
    assert_eq!(partial.glow_gain, defaults.glow_gain);
    // Опечатка в имени поля — ошибка, а не молчаливое умолчание.
    assert!(serde_json::from_str::<AcesSettings>(r#"{"kne_ev": 1.2}"#).is_err());
}

#[test]
fn rec2020_output_stays_in_working_space_without_clipping_the_wide_gamut() {
    // Насыщенный синий Rec. 2020 сцены: ACES сжимает его, но результат остаётся конечным и неотрицательным в W
    // после общего шага выхода (sRGB) — яркий цвет не вылетает за 1.
    let base = default_base();
    let blue = base.map_scene_pixel([0.0, 0.0, 0.5], 1.0);
    assert!(blue.iter().all(|c| c.is_finite()), "{blue:?}");
    let output = base.to_output(blue);
    assert!(output.iter().all(|c| (0.0..=1.0).contains(c)), "{output:?}");
    assert!(
        output[2] > output[0] && output[2] > output[1],
        "синий остался синим: {output:?}"
    );
}
