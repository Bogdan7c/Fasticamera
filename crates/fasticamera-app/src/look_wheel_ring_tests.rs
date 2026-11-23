//! Тесты цветов ободка колёс: стопы идут по кругу OKLCh от +a против часовой, число — из конфига, кривой конфиг
//! отвергается, факты для экрана несут кольцо.

use isp_model::color::chromaticity::Primaries;
use isp_model::color::oklab_working::OklabWorkspace;
use isp_model::transfer::srgb_decode;
use look_contract::{MIN_WHEEL_RING_STOPS, WheelHueRing};
use render_loop::config::RenderLoopConfig;

use super::*;
use crate::look_facts::LookFactsCatalog;
use crate::runtime_config::{ConfigError, RuntimeConfig};

fn workspace() -> OklabWorkspace {
    OklabWorkspace::new(&Primaries::SRGB).expect("матрицы sRGB строятся")
}

fn ring(style: &WheelRingStyle) -> WheelHueRing {
    wheel_ring_for_style(Some(&workspace()), style)
}

/// Возвращает оттенок цвета в градусах (0° = +a, против часовой) и его координаты (a, b) в OKLab.
fn lab_ab_of(srgb: [u8; 3]) -> (f32, f32) {
    let linear = srgb.map(|channel| srgb_decode(f32::from(channel) / 255.0));
    let [_, a, b] = workspace().to_lab(linear);
    (a, b)
}

fn hue_of(srgb: [u8; 3]) -> f32 {
    let (a, b) = lab_ab_of(srgb);
    b.atan2(a).to_degrees().rem_euclid(360.0)
}

/// Угловое расстояние между двумя оттенками, 0…180.
fn hue_gap(left: f32, right: f32) -> f32 {
    let gap = (left - right).rem_euclid(360.0);
    gap.min(360.0 - gap)
}

#[test]
fn the_ring_has_exactly_the_configured_number_of_stops() {
    for stops in [MIN_WHEEL_RING_STOPS, 36, 72, 360] {
        let style = WheelRingStyle {
            stops,
            ..WheelRingStyle::default()
        };
        assert_eq!(ring(&style).stops().len(), stops);
    }
}

#[test]
fn stops_are_laid_out_counter_clockwise_from_plus_a() {
    let style = WheelRingStyle::default();
    let stops = ring(&style).stops().to_vec();
    let count = stops.len();
    // Стоп 0: «+a» (красно-пурпурный) — a > 0, b ≈ 0.
    let (a0, b0) = lab_ab_of(stops[0].srgb);
    assert!(a0 > 0.05 && b0.abs() < a0 / 2.0, "стоп 0: a={a0}, b={b0}");
    // Четверть круга: «+b» (жёлтый) — b > 0, a ≈ 0.
    let (a1, b1) = lab_ab_of(stops[count / 4].srgb);
    assert!(b1 > 0.05 && a1.abs() < b1 / 2.0, "стоп N/4: a={a1}, b={b1}");
    // Половина: «−a» (зелёный); три четверти: «−b» (синий).
    assert!(lab_ab_of(stops[count / 2].srgb).0 < -0.05);
    assert!(lab_ab_of(stops[3 * count / 4].srgb).1 < -0.05);
}

/// Допуск оттенка, градусы. Канал u8 округляется на ±0,5/255 в sRGB; в OKLab это сдвиг до ≈ 0,003 по a/b
/// (крутой участок гаммы у тёмных каналов даёт больше). При цветности ≥ 0,1 это ≤ asin(0,003/0,1) ≈ 1,7°;
/// берём 3° с запасом. Обрезка каналов, которой было до 25°, под такой допуск не проходит.
const HUE_TOLERANCE_DEGREES: f32 = 3.0;

fn chroma_of(srgb: [u8; 3]) -> f32 {
    let (a, b) = lab_ab_of(srgb);
    a.hypot(b)
}

#[test]
fn every_stop_hue_is_exact_up_to_u8_quantisation_for_every_stop_count() {
    for stops in [MIN_WHEEL_RING_STOPS, 36, 72, 180, 360] {
        let style = WheelRingStyle {
            stops,
            ..WheelRingStyle::default()
        };
        let ring = ring(&style);
        for (index, stop) in ring.stops().iter().enumerate() {
            let expected = WheelHueRing::hue_degrees_of_stop(stops, index);
            let gap = hue_gap(hue_of(stop.srgb), expected);
            assert!(
                gap <= HUE_TOLERANCE_DEGREES,
                "N={stops}, стоп {index}: оттенок {} вместо {expected} (разница {gap}°)",
                hue_of(stop.srgb)
            );
        }
    }
}

#[test]
fn stop_chroma_never_exceeds_the_style_and_easy_hues_keep_it_whole() {
    let style = WheelRingStyle::default();
    let ring = ring(&style);
    for (index, stop) in ring.stops().iter().enumerate() {
        // Запас 0,004 на квантование u8.
        assert!(chroma_of(stop.srgb) <= style.chroma + 0.004, "стоп {index}");
    }
    // Лёгкий оттенок (зелёно-голубой, ≈ 180°): охват не мешает, цветность не занижена зря.
    let easy = ring.stops()[ring.stops().len() / 2];
    assert!(
        (chroma_of(easy.srgb) - style.chroma).abs() < 0.004,
        "цветность {} вместо {}",
        chroma_of(easy.srgb),
        style.chroma
    );
}

#[test]
fn hard_hues_lose_chroma_but_keep_hue() {
    // Очень яркие цвета не помещаются в sRGB: цветность падает, оттенок остаётся.
    let style = WheelRingStyle {
        chroma: MAX_SWATCH_CHROMA,
        ..WheelRingStyle::default()
    };
    let ring = ring(&style);
    let lowest = ring
        .stops()
        .iter()
        .map(|stop| chroma_of(stop.srgb))
        .fold(f32::MAX, f32::min);
    assert!(lowest < MAX_SWATCH_CHROMA - 0.05, "поиск не снизил цветность: {lowest}");
    for (index, stop) in ring.stops().iter().enumerate() {
        let expected = WheelHueRing::hue_degrees_of_stop(ring.stops().len(), index);
        assert!(
            hue_gap(hue_of(stop.srgb), expected) <= HUE_TOLERANCE_DEGREES,
            "стоп {index}"
        );
    }
}

#[test]
fn style_lightness_changes_the_ring_colours() {
    let bright = ring(&WheelRingStyle::default());
    let dark = ring(&WheelRingStyle {
        lightness: 0.4,
        ..WheelRingStyle::default()
    });
    let sum = |ring: &WheelHueRing| -> u32 { ring.stops().iter().flat_map(|stop| stop.srgb).map(u32::from).sum() };
    assert!(sum(&dark) < sum(&bright));
}

#[test]
fn out_of_range_style_in_code_still_gives_a_valid_ring() {
    // Обход `validate` не паникует и не отдаёт экрану негодное кольцо.
    let tiny = ring(&WheelRingStyle {
        stops: 0,
        ..WheelRingStyle::default()
    });
    assert_eq!(tiny.stops().len(), MIN_WHEEL_RING_STOPS);
    let huge = ring(&WheelRingStyle {
        stops: usize::MAX,
        ..WheelRingStyle::default()
    });
    assert_eq!(huge.stops().len(), MAX_WHEEL_RING_STOPS);
}

#[test]
fn style_validation_accepts_defaults_and_rejects_bad_values() {
    assert_eq!(WheelRingStyle::default().validate(), Ok(()));
    for stops in [0, MIN_WHEEL_RING_STOPS - 1, MAX_WHEEL_RING_STOPS + 1] {
        let style = WheelRingStyle {
            stops,
            ..WheelRingStyle::default()
        };
        assert_eq!(style.validate(), Err(WheelRingStyleError::Stops { value: stops }));
    }
    for lightness in [0.0, -0.1, 1.01, f32::NAN] {
        let style = WheelRingStyle {
            lightness,
            ..WheelRingStyle::default()
        };
        assert!(
            matches!(style.validate(), Err(WheelRingStyleError::Lightness { .. })),
            "{lightness}"
        );
    }
    for chroma in [-0.01, MAX_SWATCH_CHROMA + 0.01, f32::NAN] {
        let style = WheelRingStyle {
            chroma,
            ..WheelRingStyle::default()
        };
        assert!(
            matches!(style.validate(), Err(WheelRingStyleError::Chroma { .. })),
            "{chroma}"
        );
    }
}

#[test]
fn config_section_sets_the_stop_count_and_a_broken_one_rejects_the_whole_config() {
    let parse = |section: &str| {
        RuntimeConfig::from_json(&format!(
            r#"{{"build_type":"debug","log_filter":"info","camera_screen":{{"wheel_ring":{section}}}}}"#
        ))
    };
    let config = parse(r#"{"stops":36}"#).expect("конфиг");
    assert_eq!(config.camera_screen().wheel_ring.stops, 36);
    assert_eq!(
        config.camera_screen().wheel_ring.lightness,
        WheelRingStyle::default().lightness
    );

    let error = parse(r#"{"stops":3}"#).expect_err("мало стопов");
    assert!(
        matches!(
            &error,
            ConfigError::CameraScreen(crate::camera_screen_config::CameraScreenConfigError::WheelRing(
                WheelRingStyleError::Stops { value: 3 }
            ))
        ),
        "{error}"
    );
    assert!(parse(r#"{"chroma":9.0}"#).is_err());
    assert!(parse(r#"{"stopz":36}"#).is_err(), "опечатка в имени поля отвергается");
}

#[test]
fn catalog_carries_the_default_ring_and_the_configured_one_replaces_it() {
    let config = RenderLoopConfig::default();
    let catalog = LookFactsCatalog::from_render_config(&config, &crate::look_facts::BandSwatchStyle::default());
    assert_eq!(catalog.wheel_ring().stops().len(), WheelRingStyle::default().stops);
    let custom = catalog.with_wheel_ring_style(&WheelRingStyle {
        stops: 24,
        ..WheelRingStyle::default()
    });
    assert_eq!(custom.wheel_ring().stops().len(), 24);
}

#[test]
fn near_white_lightness_is_rejected_because_hues_would_turn_gray() {
    let style = WheelRingStyle {
        lightness: 0.98,
        ..WheelRingStyle::default()
    };
    assert!(
        matches!(
            style.validate(),
            Err(WheelRingStyleError::LightnessLosesHues { lightness, fitting_chroma, .. })
                if lightness == 0.98 && fitting_chroma < MIN_DISTINCT_STOP_CHROMA
        ),
        "{:?}",
        style.validate()
    );
    let near_black = WheelRingStyle {
        lightness: 0.01,
        ..WheelRingStyle::default()
    };
    assert!(matches!(
        near_black.validate(),
        Err(WheelRingStyleError::LightnessLosesHues { .. })
    ));
    assert_eq!(WheelRingStyle::default().validate(), Ok(()));
}

#[test]
fn a_gray_ring_by_choice_of_zero_chroma_is_still_allowed() {
    let style = WheelRingStyle {
        chroma: 0.0,
        lightness: 0.98,
        ..WheelRingStyle::default()
    };
    assert_eq!(style.validate(), Ok(()));
}

#[test]
fn the_ring_is_built_once_from_the_final_style() {
    let catalog = LookFactsCatalog::from_render_config(
        &RenderLoopConfig::default(),
        &crate::look_facts::BandSwatchStyle::default(),
    );
    assert!(!catalog.wheel_ring_is_built(), "from_render_config кольцо не считает");
    let catalog = catalog.with_wheel_ring_style(&WheelRingStyle {
        stops: 24,
        ..WheelRingStyle::default()
    });
    assert!(!catalog.wheel_ring_is_built(), "смена стиля кольцо не считает");
    let first: *const WheelHueRing = catalog.wheel_ring();
    assert!(catalog.wheel_ring_is_built());
    assert_eq!(catalog.wheel_ring().stops().len(), 24, "кольцо — по итоговому стилю");
    assert!(
        std::ptr::eq(first, catalog.wheel_ring()),
        "повторная выдача — то же кольцо, без пересчёта"
    );
}
