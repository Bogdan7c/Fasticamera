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
        (256, 256, 256)
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
                    trace_saturation_count: 0,
                    ..RenderSettings::default()
                },
                ..defaults.clone()
            },
            |e| matches!(e, ScopeSettingsError::Render(RenderSettingsError::ZeroSaturationCount)),
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
            waveform_color: [0.0, 2.0, 0.0],
            ..valid.clone()
        }),
        RenderSettingsError::ColorOutOfRange { .. }
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
        (256, 256, 1)
    );
    assert_eq!(parade.shape().planes(), 3);
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
