use camera_contract::geometry::PixelSize;

use super::{InvalidToneCurve, ToneCurve, ToneCurveSettings, apply_tone_curve};
use crate::image::{DisplayGamut, SceneInGamutImage, space};

fn default_curve() -> ToneCurve {
    ToneCurve::resolve(&ToneCurveSettings::default()).unwrap()
}

/// Оттенок в шестигранной модели HSV, градусы (для проверки сохранения оттенка).
fn hexcone_hue_degrees(pixel: [f32; 3]) -> f32 {
    let [red, green, blue] = pixel;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let span = max - min;
    assert!(span > 0.0, "у серого нет оттенка");
    let sector = if max == red {
        ((green - blue) / span).rem_euclid(6.0)
    } else if max == green {
        (blue - red) / span + 2.0
    } else {
        (red - green) / span + 4.0
    };
    sector * 60.0
}

#[test]
fn middle_gray_and_white_land_where_configured() {
    let settings = ToneCurveSettings::default();
    let curve = ToneCurve::resolve(&settings).unwrap();
    assert!((curve.map_channel(settings.scene_middle_gray) - settings.display_middle_gray).abs() < 1e-5);
    assert!((curve.map_channel(settings.scene_white) - 1.0).abs() < 1e-6);
    assert_eq!(curve.map_channel(0.0), 0.0);
    assert_eq!(curve.map_channel(-0.2), 0.0, "отрицательный свет — чёрный");
    assert_eq!(curve.map_channel(3.0), 1.0, "ярче белого — белый");
}

#[test]
fn curve_is_strictly_increasing_inside_range() {
    let curve = default_curve();
    let mut previous = 0.0;
    for step in 1..=1000 {
        let value = curve.map_channel(step as f32 / 1000.0);
        assert!(value > previous, "не возрастает на шаге {step}");
        previous = value;
    }
}

#[test]
fn contrast_changes_slope_around_middle_gray() {
    let soft = ToneCurve::resolve(&ToneCurveSettings {
        contrast: 1.0,
        ..ToneCurveSettings::default()
    })
    .unwrap();
    let hard = ToneCurve::resolve(&ToneCurveSettings {
        contrast: 1.8,
        ..ToneCurveSettings::default()
    })
    .unwrap();
    let gray = ToneCurveSettings::default().scene_middle_gray;
    // Полступени вокруг серого: у жёсткой кривой перепад больше.
    let spread = |curve: &ToneCurve| curve.map_channel(gray * 1.41) - curve.map_channel(gray / 1.41);
    assert!(
        spread(&hard) > spread(&soft) * 1.3,
        "{} против {}",
        spread(&hard),
        spread(&soft)
    );
}

#[test]
fn gray_stays_gray() {
    let curve = default_curve();
    let [red, green, blue] = curve.map_pixel([0.3, 0.3, 0.3]);
    assert_eq!(red, green);
    assert_eq!(green, blue);
}

#[test]
fn full_hue_preservation_keeps_hexcone_hue_of_bright_orange() {
    let curve = default_curve();
    // Яркий оранжевый (закат): по каналам он заметно желтеет.
    let orange = [0.9, 0.35, 0.05];
    let per_channel = orange.map(|channel| curve.map_channel(channel));
    let preserved = curve.map_pixel(orange);
    let input_hue = hexcone_hue_degrees(orange);
    assert!((hexcone_hue_degrees(preserved) - input_hue).abs() < 0.01);
    assert!(
        hexcone_hue_degrees(per_channel) - input_hue > 3.0,
        "по каналам оттенок уходит в жёлтый"
    );
    // Яркость и уход к белому — от кривой по каналам: max и min те же.
    assert_eq!(preserved[0], per_channel[0]);
    assert_eq!(preserved[2], per_channel[2]);
}

#[test]
fn zero_hue_preservation_is_plain_per_channel_curve() {
    let curve = ToneCurve::resolve(&ToneCurveSettings {
        hue_preservation: 0.0,
        ..ToneCurveSettings::default()
    })
    .unwrap();
    let pixel = [0.7, 0.2, 0.4];
    assert_eq!(curve.map_pixel(pixel), pixel.map(|channel| curve.map_channel(channel)));
}

#[test]
fn stage_converts_scene_to_display_with_same_gamut() {
    let image = SceneInGamutImage::from_pixels(
        PixelSize::new(2, 1),
        vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
        space::SceneInGamut {
            gamut: DisplayGamut::DisplayP3,
        },
    )
    .unwrap();
    let display = apply_tone_curve(&image, &default_curve());
    assert_eq!(display.space().gamut, DisplayGamut::DisplayP3);
    assert_eq!(display.pixel(0, 0), [0.0; 3]);
    for channel in display.pixel(1, 0) {
        assert!((channel - 1.0).abs() < 1e-6);
    }
}

#[test]
fn contradictory_settings_are_rejected() {
    let base = ToneCurveSettings::default();
    assert!(matches!(
        ToneCurve::resolve(&ToneCurveSettings { contrast: 0.0, ..base }),
        Err(InvalidToneCurve::InvalidContrast { .. })
    ));
    assert!(matches!(
        ToneCurve::resolve(&ToneCurveSettings {
            scene_middle_gray: 1.5,
            ..base
        }),
        Err(InvalidToneCurve::SceneMiddleGrayOutOfRange { .. })
    ));
    assert!(matches!(
        ToneCurve::resolve(&ToneCurveSettings {
            display_middle_gray: 1.0,
            ..base
        }),
        Err(InvalidToneCurve::DisplayMiddleGrayOutOfRange { .. })
    ));
    // Серый сцены 0,5 при контрасте 1: на экране он не может стать темнее 0,5.
    assert!(matches!(
        ToneCurve::resolve(&ToneCurveSettings {
            contrast: 1.0,
            scene_middle_gray: 0.5,
            display_middle_gray: 0.3,
            ..base
        }),
        Err(InvalidToneCurve::DisplayMiddleGrayUnreachable { .. })
    ));
    assert!(matches!(
        ToneCurve::resolve(&ToneCurveSettings {
            hue_preservation: 1.5,
            ..base
        }),
        Err(InvalidToneCurve::HuePreservationOutOfRange { .. })
    ));
}

#[test]
fn nan_light_is_black_not_white() {
    let curve = default_curve();
    assert_eq!(curve.map_channel(f32::NAN), 0.0);
    assert_eq!(crate::transfer::srgb_encode(f32::NAN), 0.0);
    assert_eq!(curve.map_pixel([0.5, f32::NAN, 0.2]), [0.0; 3]);
}
