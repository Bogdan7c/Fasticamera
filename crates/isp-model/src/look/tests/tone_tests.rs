use std::sync::Arc;

use super::*;
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;
use crate::look::model::LookModelSettings;
use crate::look::validation::{ToneEdgeError, validate_tone_edges};
use crate::look::{LookControls, tone_amounts};
use crate::tone::agx::AgxBase;
use crate::tone::agx_settings::AgxSettings;
use crate::tone::base::ToneBase;
use crate::tone::knobs::{ToneKnob, ToneKnobUnsupported, ToneModelKind};
use crate::tone::{ToneCurve, ToneCurveSettings};

/// Кривая AgX после ручек вида `settings` (через единый шов основы).
fn agx_after(settings: &LookSettings) -> (AgxBase, AgxSettings) {
    let model = LookModelSettings::default();
    let knobbed = ToneBase::default().with_knobs(&ToneCurveSettings::default(), &tone_amounts(&model, settings));
    assert!(knobbed.unsupported.is_empty());
    let ToneBase::Agx(adjusted) = knobbed.base else {
        panic!("основа должна остаться AgX");
    };
    let base = AgxBase::resolve(&adjusted, DisplayGamut::Srgb).unwrap();
    ((*base).clone(), adjusted)
}

fn curve_at(base: &AgxBase, settings: &AgxSettings, ev: f32) -> f32 {
    base.curve_channel(settings.scene_middle_gray * ev.exp2(), 1.0)
}

/// Наклон в `e = 0` конечной разностью (на ступень).
fn slope_at_grey(base: &AgxBase, settings: &AgxSettings) -> f32 {
    let step = 0.01;
    (curve_at(base, settings, step) - curve_at(base, settings, -step)) / (2.0 * step)
}

#[test]
fn contrast_scales_the_slope_at_grey_by_the_range_formula_and_keeps_the_grey_value() {
    let model = LookModelSettings::default();
    let (base0, s0) = agx_after(&LookSettings::NEUTRAL);
    for k in [-1.0_f32, 1.0] {
        let (base, settings) = agx_after(&settings_with(|s| s.contrast = knob(k)));
        let expected = (k * model.contrast_range_log2).exp2();
        let ratio = slope_at_grey(&base, &settings) / slope_at_grey(&base0, &s0);
        assert!(
            (ratio - expected).abs() < 0.01,
            "k={k}: наклон ×{ratio}, ожидали ×{expected}"
        );
        assert!(
            (curve_at(&base, &settings, 0.0) - curve_at(&base0, &s0, 0.0)).abs() < 1e-6,
            "серый сдвинулся"
        );
    }
}

#[test]
fn midtones_move_the_grey_the_scene_lands_on_by_the_configured_stops() {
    let model = LookModelSettings::default();
    for k in [-1.0_f32, 1.0] {
        let (base, settings) = agx_after(&settings_with(|s| s.midtones = knob(k)));
        let display_grey = 0.2_f32 * (k * model.midtones_range_ev).exp2();
        let landed = curve_at(&base, &settings, 0.0).powf(settings.display_gamma);
        assert!(
            (landed - display_grey).abs() < 1e-5,
            "k={k}: серый {landed}, ожидали {display_grey}"
        );
    }
}

#[test]
fn shadows_change_the_toe_only_and_highlights_the_shoulder_only() {
    let (base0, s0) = agx_after(&LookSettings::NEUTRAL);
    let grey = curve_at(&base0, &s0, 0.0);
    let slope = slope_at_grey(&base0, &s0);
    for (k, sign) in [(1.0_f32, 1.0_f32), (-1.0, -1.0)] {
        let (base, settings) = agx_after(&settings_with(|s| s.shadows = knob(k)));
        // «+ тени» = светлее тени: ниже серого кривая выше, и она сдвигается заметно.
        for ev in [-5.0, -3.0, -1.5] {
            let delta = curve_at(&base, &settings, ev) - curve_at(&base0, &s0, ev);
            assert!(delta * sign > 0.004, "тени k={k} e={ev}: Δ={delta}");
        }
        // Середина (значение и наклон) и всё плечо не тронуты.
        assert!((curve_at(&base, &settings, 0.0) - grey).abs() < 1e-6);
        assert!((slope_at_grey(&base, &settings) - slope).abs() < 2e-3);
        for ev in [1.0, 2.0, 3.0] {
            assert_eq!(
                curve_at(&base, &settings, ev),
                curve_at(&base0, &s0, ev),
                "света тронуты тенями e={ev}"
            );
        }

        let (base, settings) = agx_after(&settings_with(|s| s.highlights = knob(k)));
        for ev in [2.0, 3.0] {
            let delta = curve_at(&base, &settings, ev) - curve_at(&base0, &s0, ev);
            assert!(delta * sign > 0.002, "света k={k} e={ev}: Δ={delta}");
        }
        assert!((curve_at(&base, &settings, 0.0) - grey).abs() < 1e-6);
        assert!((slope_at_grey(&base, &settings) - slope).abs() < 2e-3);
        for ev in [-5.0, -3.0, -1.5] {
            assert_eq!(
                curve_at(&base, &settings, ev),
                curve_at(&base0, &s0, ev),
                "тени тронуты светами e={ev}"
            );
        }
    }
}

#[test]
fn legacy_base_takes_contrast_and_midtones_and_reports_shadows_and_highlights_as_unsupported() {
    let model = LookModelSettings::default();
    let settings = settings_with(|s| {
        s.contrast = knob(1.0);
        s.midtones = knob(-1.0);
        s.shadows = knob(0.7);
        s.highlights = knob(-0.3);
    });
    let knobbed = ToneBase::Legacy.with_knobs(&ToneCurveSettings::default(), &tone_amounts(&model, &settings));
    let defaults = ToneCurveSettings::default();
    assert!((knobbed.legacy_curve.contrast - defaults.contrast * model.contrast_range_log2.exp2()).abs() < 1e-6);
    assert!(
        (knobbed.legacy_curve.display_middle_gray - defaults.display_middle_gray * (-model.midtones_range_ev).exp2())
            .abs()
            < 1e-6
    );
    assert_eq!(
        knobbed.unsupported,
        vec![
            ToneKnobUnsupported {
                knob: ToneKnob::Shadows,
                model: ToneModelKind::Legacy
            },
            ToneKnobUnsupported {
                knob: ToneKnob::Highlights,
                model: ToneModelKind::Legacy
            },
        ]
    );
    assert_eq!(knobbed.base, ToneBase::Legacy);
    // Кривая со сдвинутыми ручками строится и отличается от исходной.
    let changed = ToneCurve::resolve(&knobbed.legacy_curve).unwrap();
    let original = ToneCurve::resolve(&defaults).unwrap();
    assert_ne!(changed.map_channel(0.03), original.map_channel(0.03));
}

#[test]
fn neutral_tone_knobs_return_the_settings_unchanged() {
    let knobbed = ToneBase::default().with_knobs(
        &ToneCurveSettings::default(),
        &tone_amounts(&LookModelSettings::default(), &LookSettings::NEUTRAL),
    );
    assert_eq!(knobbed.base, ToneBase::default());
    assert_eq!(knobbed.legacy_curve, ToneCurveSettings::default());
    assert!(knobbed.unsupported.is_empty());
}

#[test]
fn the_default_model_is_usable_at_every_extreme_knob_corner_and_both_edges_of_the_ae_lift() {
    let model = LookModelSettings::default();
    for output in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        validate_tone_edges(&model, &ToneBase::default(), &ToneCurveSettings::default(), output, 0.5).unwrap();
        validate_tone_edges(&model, &ToneBase::Legacy, &ToneCurveSettings::default(), output, 0.5).unwrap();
    }
}

#[test]
fn every_registered_model_is_checked_at_the_edges_and_unusable_corners_name_their_model() {
    use crate::tone::aces::AcesSettings;
    use crate::tone::filmic::FilmicSettings;
    let model = LookModelSettings::default();
    let legacy = ToneCurveSettings::default();
    for output in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        for base in [
            ToneBase::Agx(AgxSettings::default()),
            ToneBase::Aces(AcesSettings::default()),
            ToneBase::Filmic(FilmicSettings::default()),
        ] {
            validate_tone_edges(&model, &base, &legacy, output, 0.5).unwrap_or_else(|error| {
                panic!(
                    "{:?}: умолчания модели не проходят проверку краёв: {error}",
                    base.kind()
                )
            });
        }
    }
    // Тесная ось теней и света у ACES: на краю ручки форма разворачивает ось экспозиции — отказ называет модель и край.
    let narrow = ToneBase::Aces(AcesSettings {
        shape_width_ev: 0.5,
        ..AcesSettings::default()
    });
    let error = validate_tone_edges(&model, &narrow, &legacy, DisplayGamut::Srgb, 0.5).unwrap_err();
    let ToneEdgeError::CurveUnusable(reason) = &error else {
        panic!("ожидали CurveUnusable: {error:?}");
    };
    assert!(reason.contains("основа ACES") && reason.contains("тени"), "{reason}");
    // Плоский filmic: наклон слишком мал, чтобы плечо дошло до белого при подтяжке AE.
    let shallow = ToneBase::Filmic(FilmicSettings {
        slope: 0.1,
        ..FilmicSettings::default()
    });
    let error = validate_tone_edges(&model, &shallow, &legacy, DisplayGamut::Srgb, 0.5).unwrap_err();
    let ToneEdgeError::CurveUnusable(reason) = &error else {
        panic!("ожидали CurveUnusable: {error:?}");
    };
    assert!(reason.contains("основа filmic"), "{reason}");
}

#[test]
fn an_oversized_range_is_rejected_at_the_edge_with_a_readable_reason() {
    // Контраст вниз на 1 EV наклона: плечо перестаёт доходить до белого (r₁ ≤ 1).
    let model = LookModelSettings {
        contrast_range_log2: 1.0,
        ..LookModelSettings::default()
    };
    let error = validate_tone_edges(
        &model,
        &ToneBase::default(),
        &ToneCurveSettings::default(),
        DisplayGamut::Srgb,
        0.5,
    )
    .unwrap_err();
    let ToneEdgeError::CurveUnusable(reason) = &error else {
        panic!("ожидали CurveUnusable: {error:?}");
    };
    assert!(reason.contains("контраст -1") && reason.contains("плечо"), "{reason}");
    // Без остаточной подтяжки AE тот же диапазон может проходить, но с подтяжкой — нет: край AE учитывается.
    let tight = LookModelSettings {
        contrast_range_log2: 0.3,
        ..LookModelSettings::default()
    };
    let at_zero = validate_tone_edges(
        &tight,
        &ToneBase::default(),
        &ToneCurveSettings::default(),
        DisplayGamut::Srgb,
        0.0,
    );
    let at_half = validate_tone_edges(
        &tight,
        &ToneBase::default(),
        &ToneCurveSettings::default(),
        DisplayGamut::Srgb,
        0.5,
    );
    assert!(at_zero.is_ok() && at_half.is_err(), "{at_zero:?} / {at_half:?}");
    // Неверная модель — отдельная ошибка.
    let broken = LookModelSettings {
        saturation_range: 7.0,
        ..LookModelSettings::default()
    };
    assert!(matches!(
        validate_tone_edges(
            &broken,
            &ToneBase::default(),
            &ToneCurveSettings::default(),
            DisplayGamut::Srgb,
            0.0
        ),
        Err(ToneEdgeError::Model(_))
    ));
}

#[test]
fn ae_lift_and_knobs_compose_through_the_real_resolution() {
    use crate::tone::base::ResolvedToneBase;
    let controls = |look: LookControls, gain: ExposureGain| {
        let mut controls = super::white_tests::controls();
        controls.exposure_gain = gain;
        controls.look = look;
        controls
    };
    let sensor = super::white_tests::sensor();
    let frame = super::white_tests::frame(&sensor);
    let gain = ExposureGain::from_ev(0.5).unwrap();
    let neutral = crate::params::IspParams::resolve(&sensor, &frame, &controls(LookControls::default(), gain)).unwrap();
    let contrasty = LookControls::new(
        settings_with(|s| s.contrast = knob(1.0)),
        Arc::new(LookModelSettings::default()),
    );
    let touched = crate::params::IspParams::resolve(&sensor, &frame, &controls(contrasty, gain)).unwrap();
    let (ResolvedToneBase::Agx(a), ResolvedToneBase::Agx(b)) = (&neutral.tone_base, &touched.tone_base) else {
        panic!("ожидали AgX");
    };
    let (a, b) = (a.parameters(), b.parameters());
    assert!((b.slope / a.slope - 0.1_f32.exp2()).abs() < 1e-5);
    // Подтяжка AE осталась на месте: серый сцены тот же у обоих.
    assert_eq!(a.grey, b.grey);
    assert!(touched.look.report.unsupported_tone_knobs.is_empty());
}
