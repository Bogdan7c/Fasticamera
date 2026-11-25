//! Настройки ACES: форма теней и света, наклон, серый экрана, края подтяжки AE, ошибки и serde.
//!
//! Раньше эти числа крутили ручки тона слоя вида через шов основы; с 14l ручки — отдельная стадия
//! (`look::tone_shape`), а здесь настройки модели меняются напрямую: то, что умела форма, остаётся проверенным.

use super::AcesBase;
use super::settings::{AcesSettings, InvalidAces};
use super::tests::{base_for, default_base, grey_at, grey_pixel, tone_curve};
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;

// ---- Форма кривой ----

fn grey_output_at(settings: &AcesSettings, e: f32) -> f32 {
    base_for(settings).map_scene_pixel(grey_pixel(grey_at(settings, e)), 1.0)[0]
}

/// Наклон выхода по оси экспозиции вокруг `e` (центральная разность по `±step` ступени).
fn slope_at(settings: &AcesSettings, e: f32, step: f32) -> f32 {
    (grey_output_at(settings, e + step) - grey_output_at(settings, e - step)) / (2.0 * step)
}

/// Умолчания с одним изменённым полем формы.
fn with_shape(shadows_ev: f32, highlights_ev: f32) -> AcesSettings {
    AcesSettings {
        shadows_ev,
        highlights_ev,
        ..AcesSettings::default()
    }
}

#[test]
fn contrast_setting_scales_the_slope_around_the_grey_and_keeps_the_grey() {
    let defaults = AcesSettings::default();
    let steeper = AcesSettings {
        contrast: defaults.contrast * 0.3_f32.exp2(),
        ..defaults
    };
    assert!((grey_output_at(&steeper, 0.0) - defaults.display_middle_gray).abs() < 3e-6);
    let ratio = slope_at(&steeper, 0.0, 0.02) / slope_at(&defaults, 0.0, 0.02);
    assert!(
        (ratio - 0.3_f32.exp2()).abs() < 0.02,
        "наклон вырос в {ratio}, ждали {}",
        0.3_f32.exp2()
    );
    let flatter = AcesSettings {
        contrast: defaults.contrast * (-0.3_f32).exp2(),
        ..defaults
    };
    assert!(slope_at(&flatter, 0.0, 0.02) < slope_at(&defaults, 0.0, 0.02));
}

#[test]
fn display_grey_setting_moves_the_grey_exactly() {
    for amount in [0.3_f32, -0.3] {
        let settings = AcesSettings {
            display_middle_gray: AcesSettings::default().display_middle_gray * amount.exp2(),
            ..AcesSettings::default()
        };
        let expected = AcesSettings::default().display_middle_gray * amount.exp2();
        assert!(
            (grey_output_at(&settings, 0.0) - expected).abs() < 3e-6,
            "{amount}: {}",
            grey_output_at(&settings, 0.0)
        );
    }
}

#[test]
fn shadows_ev_lifts_only_the_shadows() {
    let defaults = AcesSettings::default();
    let lifted = with_shape(1.2, 0.0);
    let darkened = with_shape(-1.2, 0.0);
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
fn highlights_ev_lifts_only_the_highlights() {
    let defaults = AcesSettings::default();
    let lifted = with_shape(0.0, 1.2);
    let darkened = with_shape(0.0, -1.2);
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
fn curve_builds_keeps_white_exact_and_grows_at_the_ae_gain_edges() {
    // Подтяжка AE ±0,5 ступени (решение 72) — единственное, что после 14l двигает серый кривой на кадре; формы
    // теней и света берём крайние из допустимых настроек, чтобы край подтяжки проверялся на самой тонкой кривой.
    for shape in [-1.2_f32, 0.0, 1.2] {
        for gain in [0.5_f32.exp2().recip(), 1.0, 0.5_f32.exp2()] {
            let settings = with_shape(shape, -shape).with_exposure_gain(ExposureGain::new(gain).unwrap());
            let label = format!("форма {shape} k={gain}");
            let base =
                AcesBase::resolve(&settings, DisplayGamut::Srgb).unwrap_or_else(|error| panic!("{label}: {error}"));
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
