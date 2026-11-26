//! Настройки: значения по умолчанию, проверка, serde; решение прибора из запроса экрана.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    RenderSettings, RenderSettingsError, ResolveError, ResolvedScope, SampleStride, ScopeModelSettings, ScopeRule,
    ScopeSettingsError,
};

/// Проверка, что ошибка именно та, что ожидалась для данного значения.
type ErrorCheck = fn(&ScopeSettingsError) -> bool;

fn request(kind: ScopeKind, axes: VectorscopeAxes) -> ScopeRequest {
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(300, 200).unwrap(),
    }
}

#[test]
fn defaults_are_valid_and_match_documented_estimates() {
    let settings = ScopeModelSettings::default();
    assert_eq!(settings.validate(), Ok(()));
    assert_eq!(
        (settings.trace_columns, settings.trace_levels, settings.vectorscope_size),
        (512, 256, 256)
    );
    assert_eq!(settings.sample_stride.get(), 2);
    assert_eq!(settings.ycbcr_gain, 1.0);
    assert_eq!(settings.oklab_range, 0.4);
}

#[test]
fn every_invalid_value_reports_its_own_error() {
    let defaults = ScopeModelSettings::default();
    let cases: Vec<(ScopeModelSettings, ErrorCheck)> = vec![
        (
            ScopeModelSettings {
                trace_columns: 0,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::TraceColumnsOutOfRange { columns: 0 }),
        ),
        (
            ScopeModelSettings {
                trace_levels: 2000,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::TraceLevelsOutOfRange { levels: 2000 }),
        ),
        (
            ScopeModelSettings {
                vectorscope_size: 4,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::VectorscopeSizeOutOfRange { size: 4 }),
        ),
        (
            ScopeModelSettings {
                ycbcr_gain: 0.5,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::YCbCrGainOutOfRange { .. }),
        ),
        (
            ScopeModelSettings {
                ycbcr_gain: f32::NAN,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::YCbCrGainOutOfRange { .. }),
        ),
        (
            ScopeModelSettings {
                oklab_range: 0.0,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::OklabRangeOutOfRange { .. }),
        ),
        // 4096 столбцов × 1024 уровня × 3 плоскости — за бюджет ячеек.
        (
            ScopeModelSettings {
                trace_columns: 4096,
                trace_levels: 1024,
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::GridBudget(_)),
        ),
        (
            ScopeModelSettings {
                render: RenderSettings {
                    vectorscope_lightness: 1.0,
                    ..RenderSettings::default()
                },
                ..defaults.clone()
            },
            |e| {
                matches!(
                    e,
                    ScopeSettingsError::Render(RenderSettingsError::LightnessOutOfRange { .. })
                )
            },
        ),
    ];
    for (settings, is_expected) in cases {
        let error = settings.validate().unwrap_err();
        assert!(is_expected(&error), "неожиданная ошибка: {error:?}");
    }
}

#[test]
fn render_settings_report_distinct_errors() {
    let valid = RenderSettings::default();
    assert_eq!(valid.validate(), Ok(()));
    let check = |settings: RenderSettings| settings.validate().unwrap_err();
    assert!(matches!(
        check(RenderSettings {
            trace_opacity: 1.5,
            ..valid.clone()
        }),
        RenderSettingsError::OpacityOutOfRange {
            field: "trace_opacity",
            ..
        }
    ));
    assert!(matches!(
        check(RenderSettings {
            parade_colors: [[0.0, 2.0, 0.0]; 3],
            ..valid.clone()
        }),
        RenderSettingsError::ColorOutOfRange { .. }
    ));
    for lightness in [0.0, 1.0, -0.5, 1.5] {
        assert!(
            matches!(
                check(RenderSettings {
                    vectorscope_lightness: lightness,
                    ..valid.clone()
                }),
                RenderSettingsError::LightnessOutOfRange { .. }
            ),
            "светлота {lightness}"
        );
    }
    assert!(matches!(
        check(RenderSettings {
            vectorscope_lightness: f32::NAN,
            ..valid.clone()
        }),
        RenderSettingsError::NotFinite { .. }
    ));
    assert!(matches!(
        check(RenderSettings {
            graticule_half_width_px: f32::INFINITY,
            ..valid.clone()
        }),
        RenderSettingsError::NotFinite { .. }
    ));
    assert!(matches!(
        check(RenderSettings {
            graticule_half_width_px: 40.0,
            ..valid.clone()
        }),
        RenderSettingsError::LineHalfWidthOutOfRange { .. }
    ));
    assert!(matches!(
        check(RenderSettings {
            skin_line_angle_degrees: 400.0,
            ..valid.clone()
        }),
        RenderSettingsError::SkinAngleOutOfRange { .. }
    ));
    assert!(matches!(
        check(RenderSettings {
            target_amplitude: 0.0,
            ..valid.clone()
        }),
        RenderSettingsError::TargetAmplitudeOutOfRange { .. }
    ));
    assert!(matches!(
        check(RenderSettings {
            target_box_half_cells: 100.0,
            ..valid
        }),
        RenderSettingsError::TargetBoxOutOfRange { .. }
    ));
}

#[test]
fn settings_round_trip_through_json_and_reject_unknown_fields() {
    let settings = ScopeModelSettings {
        sample_stride: SampleStride::try_new(4).unwrap(),
        ..ScopeModelSettings::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    assert_eq!(serde_json::from_str::<ScopeModelSettings>(&json).unwrap(), settings);
    // Пропущенные поля берутся из значений по умолчанию.
    let partial: ScopeModelSettings = serde_json::from_str(r#"{"trace_columns": 128}"#).unwrap();
    assert_eq!(partial.trace_columns, 128);
    assert_eq!(partial.trace_levels, 256);
    assert_eq!(partial.render, RenderSettings::default());
    assert!(serde_json::from_str::<ScopeModelSettings>(r#"{"unknown": 1}"#).is_err());
    assert!(serde_json::from_str::<ScopeModelSettings>(r#"{"sample_stride": 0}"#).is_err());
}

#[test]
fn resolve_picks_the_concrete_rule_and_shapes_from_the_request() {
    let settings = ScopeModelSettings::default();
    let resolve = |kind, axes| ResolvedScope::resolve(&request(kind, axes), &settings, DisplayGamut::Srgb).unwrap();
    let waveform = resolve(ScopeKind::Waveform, VectorscopeAxes::OklabAb);
    let parade = resolve(ScopeKind::RgbParade, VectorscopeAxes::OklabAb);
    let ycbcr = resolve(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709);
    let oklab = resolve(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb);
    assert!(matches!(waveform, ResolvedScope::Waveform(_)));
    assert!(matches!(parade, ResolvedScope::RgbParade(_)));
    assert!(matches!(ycbcr, ResolvedScope::VectorscopeYCbCr(_)));
    assert!(matches!(oklab, ResolvedScope::VectorscopeOklab(_)));
    assert_eq!(waveform.kind(), ScopeKind::Waveform);
    assert_eq!(oklab.kind(), ScopeKind::Vectorscope);
    assert_eq!(
        (
            waveform.shape().columns(),
            waveform.shape().rows(),
            waveform.shape().planes()
        ),
        (512, 256, 3)
    );
    // RGB waveform и parade — одно правило трёх плоскостей: форма сетки у них одна.
    assert_eq!(waveform.shape(), parade.shape());
    // Смена осей вектороскопа не меняет форму сетки: ресурсы те же.
    assert_eq!(ycbcr.shape(), oklab.shape());
}

#[test]
fn resolve_rejects_invalid_settings_instead_of_building_a_rule() {
    let settings = ScopeModelSettings {
        vectorscope_size: 3,
        ..ScopeModelSettings::default()
    };
    let result = ResolvedScope::resolve(
        &request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709),
        &settings,
        DisplayGamut::Srgb,
    );
    assert!(matches!(
        result,
        Err(ResolveError::Settings(ScopeSettingsError::VectorscopeSizeOutOfRange {
            size: 3
        }))
    ));
}

#[test]
fn display_gamut_changes_the_oklab_rule_but_not_ycbcr() {
    let settings = ScopeModelSettings::default();
    let oklab_request = request(ScopeKind::Vectorscope, VectorscopeAxes::OklabAb);
    let srgb = ResolvedScope::resolve(&oklab_request, &settings, DisplayGamut::Srgb).unwrap();
    let p3 = ResolvedScope::resolve(&oklab_request, &settings, DisplayGamut::DisplayP3).unwrap();
    assert_ne!(srgb, p3);
    let ycbcr_request = request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709);
    assert_eq!(
        ResolvedScope::resolve(&ycbcr_request, &settings, DisplayGamut::Srgb).unwrap(),
        ResolvedScope::resolve(&ycbcr_request, &settings, DisplayGamut::DisplayP3).unwrap()
    );
}

/// Настройки с заданной светлотой вектороскопа (маленькая сетка: тест не про размер).
fn settings_with_lightness(lightness: f32) -> ScopeModelSettings {
    let mut settings = ScopeModelSettings::default();
    settings.vectorscope_size = 64;
    settings.render.vectorscope_lightness = lightness;
    settings
}

/// Таблица цветов, которую решение обязано положить в правило: строится заново из тех же осей и светлоты.
fn expected_palette(
    axes: VectorscopeAxes,
    settings: &ScopeModelSettings,
    gamut: DisplayGamut,
) -> scope_model::PositionPalette {
    let lightness = settings.render.vectorscope_lightness;
    match axes {
        VectorscopeAxes::YCbCrBt709 => {
            scope_model::vectorscope_ycbcr::YCbCrVectorscopeRule::new(settings.vectorscope_size, settings.ycbcr_gain)
                .unwrap()
                .position_palette(lightness)
        }
        VectorscopeAxes::OklabAb => {
            scope_model::oklab_rule::OklabVectorscopeRule::new(settings.vectorscope_size, settings.oklab_range, gamut)
                .unwrap()
                .position_palette(lightness)
        }
    }
}

fn palette_of_style(scope: &ResolvedScope, settings: &RenderSettings) -> scope_model::PositionPalette {
    match scope.trace_style(settings) {
        scope_model::TraceStyle::ColorByPosition { palette } => *palette,
        other => panic!(
            "у вектороскопа ожидался способ «по месту», получено {:?}",
            other.paint()
        ),
    }
}

#[test]
fn vectorscope_palette_is_built_at_resolve_time_from_the_resolve_settings() {
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let settings = settings_with_lightness(0.6);
            let scope = ResolvedScope::resolve(&request(ScopeKind::Vectorscope, axes), &settings, gamut).unwrap();
            assert_eq!(
                palette_of_style(&scope, &settings.render),
                expected_palette(axes, &settings, gamut),
                "{axes:?} {gamut:?}"
            );
        }
    }
}

#[test]
fn resolve_with_another_lightness_gives_another_palette() {
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let dark = settings_with_lightness(0.4);
        let light = settings_with_lightness(0.8);
        let resolve = |settings: &ScopeModelSettings| {
            ResolvedScope::resolve(&request(ScopeKind::Vectorscope, axes), settings, DisplayGamut::Srgb).unwrap()
        };
        let dark_palette = palette_of_style(&resolve(&dark), &dark.render);
        let light_palette = palette_of_style(&resolve(&light), &light.render);
        assert_ne!(dark_palette, light_palette, "{axes:?}");
        assert_ne!(dark_palette.neutral(), light_palette.neutral(), "{axes:?}");
    }
}

#[test]
fn trace_style_hands_out_the_stored_palette_and_does_not_rebuild_it_from_call_settings() {
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let resolve_settings = settings_with_lightness(0.6);
        let scope = ResolvedScope::resolve(
            &request(ScopeKind::Vectorscope, axes),
            &resolve_settings,
            DisplayGamut::Srgb,
        )
        .unwrap();
        // В вызов приходит другая светлота: перестроение дало бы другую таблицу, хранимая остаётся прежней.
        let mut other_call_settings = resolve_settings.render.clone();
        other_call_settings.vectorscope_lightness = 0.9;
        assert_eq!(
            palette_of_style(&scope, &other_call_settings),
            expected_palette(axes, &resolve_settings, DisplayGamut::Srgb),
            "{axes:?}"
        );
        // Повторные вызовы дают ту же таблицу.
        assert_eq!(
            palette_of_style(&scope, &resolve_settings.render),
            palette_of_style(&scope, &resolve_settings.render)
        );
    }
}
