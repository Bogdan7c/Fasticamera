//! Проверка основ при старте (`validate_tone_edges`): модель вида годна, кривая основы строится на краях подтяжки AE.

use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;
use crate::look::model::{InvalidLookModel, LookModelSettings};
use crate::look::range::KnobRange;
use crate::look::validation::{ToneEdgeError, validate_tone_edges};
use crate::tone::ToneCurveSettings;
use crate::tone::aces::AcesSettings;
use crate::tone::agx_settings::AgxSettings;
use crate::tone::base::ToneBase;
use crate::tone::filmic::FilmicSettings;

/// Наибольшая остаточная подтяжка AE в тестах, ступеней (как в конфиге съёмки: решение 72).
const MAX_RESIDUAL_EV: f32 = 0.5;

fn defaults_of_every_base() -> [ToneBase; 4] {
    [
        ToneBase::Legacy,
        ToneBase::Agx(AgxSettings::default()),
        ToneBase::Aces(AcesSettings::default()),
        ToneBase::Filmic(FilmicSettings::default()),
    ]
}

fn check(
    model: &LookModelSettings,
    base: &ToneBase,
    legacy: &ToneCurveSettings,
    gamut: DisplayGamut,
) -> Result<(), ToneEdgeError> {
    validate_tone_edges(model, base, legacy, gamut, MAX_RESIDUAL_EV)
}

#[test]
fn the_defaults_pass_for_every_base_on_both_display_gamuts() {
    for base in defaults_of_every_base() {
        for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
            assert_eq!(
                check(
                    &LookModelSettings::default(),
                    &base,
                    &ToneCurveSettings::default(),
                    gamut
                ),
                Ok(()),
                "{:?} на {gamut:?}",
                base.kind()
            );
        }
    }
}

#[test]
fn a_broken_look_model_is_reported_as_the_model_error_for_every_base() {
    let broken = LookModelSettings {
        saturation: KnobRange { minus: 1.5, plus: 2.0 },
        ..LookModelSettings::default()
    };
    for base in defaults_of_every_base() {
        let error = check(&broken, &base, &ToneCurveSettings::default(), DisplayGamut::Srgb).unwrap_err();
        assert!(
            matches!(
                error,
                ToneEdgeError::Model(InvalidLookModel::OutOfRange {
                    field: "saturation",
                    ..
                })
            ),
            "{:?}: {error:?}",
            base.kind()
        );
    }
    // Ошибка тона вида не путается с ошибкой кривой: число тона тоже приходит как `Model`.
    let broken_tone = LookModelSettings {
        tone: crate::look::tone_shape::ToneShapeSettings {
            color_follow: 2.0,
            ..Default::default()
        },
        ..LookModelSettings::default()
    };
    assert!(matches!(
        check(
            &broken_tone,
            &ToneBase::Legacy,
            &ToneCurveSettings::default(),
            DisplayGamut::Srgb
        ),
        Err(ToneEdgeError::Model(_))
    ));
}

/// Наклон 0,15 годен без подтяжки и при +0,5 EV, но плечо AgX не вогнуто на −0,5 EV (замер по умолчаниям):
/// именно краевая проверка обязана это поймать, а не кадр посреди съёмки.
fn agx_that_breaks_only_at_the_dark_ae_edge() -> ToneBase {
    ToneBase::Agx(AgxSettings {
        slope: 0.15,
        ..AgxSettings::default()
    })
}

#[test]
fn a_base_that_builds_neutral_but_not_at_the_ae_edge_is_refused_naming_the_edge() {
    let base = agx_that_breaks_only_at_the_dark_ae_edge();
    // Без подтяжки основа действительно строится: отказ — заслуга именно краевой проверки.
    assert!(base.resolve(ExposureGain::UNITY, DisplayGamut::Srgb).is_ok());
    let error = check(
        &LookModelSettings::default(),
        &base,
        &ToneCurveSettings::default(),
        DisplayGamut::Srgb,
    )
    .unwrap_err();
    let ToneEdgeError::CurveUnusable(reason) = &error else {
        panic!("ждали CurveUnusable: {error:?}");
    };
    assert!(reason.contains("подтяжка -0.5 EV"), "{reason}");
    assert!(error.to_string().contains("подтяжка -0.5 EV"), "{error}");
    // Тот же вид без края подтяжки (0 EV) проходит: проверка смотрит именно на край.
    assert_eq!(
        validate_tone_edges(
            &LookModelSettings::default(),
            &base,
            &ToneCurveSettings::default(),
            DisplayGamut::Srgb,
            0.0
        ),
        Ok(())
    );
}

#[test]
fn the_legacy_base_checks_the_legacy_curve_settings_not_the_variant() {
    let broken_curve = ToneCurveSettings {
        contrast: 0.0,
        ..ToneCurveSettings::default()
    };
    let error = check(
        &LookModelSettings::default(),
        &ToneBase::Legacy,
        &broken_curve,
        DisplayGamut::Srgb,
    )
    .unwrap_err();
    assert!(matches!(error, ToneEdgeError::CurveUnusable(_)), "{error:?}");
    // У других основ настройки прежней кривой не читаются: тот же «сломанный» legacy им не мешает.
    assert_eq!(
        check(
            &LookModelSettings::default(),
            &ToneBase::Agx(AgxSettings::default()),
            &broken_curve,
            DisplayGamut::Srgb
        ),
        Ok(())
    );
}
