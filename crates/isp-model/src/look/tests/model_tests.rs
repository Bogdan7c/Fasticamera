use super::*;
use crate::look::bands::MAX_LOOK_BANDS;
use crate::look::model::{HueBandDefinition, InvalidLookModel, LookModelSettings};
use crate::look::range::KnobRange;

#[test]
fn default_model_is_valid_and_has_eight_bands() {
    let model = LookModelSettings::default();
    model.validate().unwrap();
    assert_eq!(model.bands.len(), 8, "идентификатор стартового набора отброшен");
    let ids: Vec<&str> = model.bands.iter().map(|band| band.id.as_str()).collect();
    assert_eq!(
        ids,
        ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"]
    );
}

#[test]
fn serde_roundtrip_keeps_the_model_and_unknown_fields_are_rejected() {
    let model = LookModelSettings::default();
    let json = serde_json::to_string(&model).unwrap();
    assert_eq!(serde_json::from_str::<LookModelSettings>(&json).unwrap(), model);
    assert!(serde_json::from_str::<LookModelSettings>(r#"{"contrast_range":1}"#).is_err());
    // Частичный конфиг дополняется умолчанием.
    let partial: LookModelSettings =
        serde_json::from_str(r#"{"warmth_mired": {"minus": 80.0, "plus": 120.0}}"#).unwrap();
    assert_eq!(
        partial.warmth_mired,
        KnobRange {
            minus: 80.0,
            plus: 120.0
        }
    );
    assert_eq!(partial.bands, model.bands);
}

#[test]
fn every_refusal_is_distinct_and_names_its_field() {
    let default = LookModelSettings::default();
    let bad_range = LookModelSettings {
        saturation: KnobRange { minus: 1.5, plus: 2.0 },
        ..default.clone()
    };
    let error = bad_range.validate().unwrap_err();
    assert!(
        matches!(
            error,
            InvalidLookModel::OutOfRange {
                field: "saturation",
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("saturation"));
    let nan = LookModelSettings {
        tint_duv: KnobRange::symmetric(f32::NAN),
        ..default.clone()
    };
    assert!(matches!(
        nan.validate(),
        Err(InvalidLookModel::OutOfRange { field: "tint_duv", .. })
    ));

    let none = LookModelSettings {
        bands: Vec::new(),
        ..default.clone()
    };
    assert_eq!(
        none.validate(),
        Err(InvalidLookModel::BandCount {
            count: 0,
            max: MAX_LOOK_BANDS
        })
    );
    let many = LookModelSettings {
        bands: (0..17)
            .map(|index| HueBandDefinition {
                id: band_id(&format!("band-{index}")),
                center_degrees: index as f32 * 20.0,
            })
            .collect(),
        ..default.clone()
    };
    assert_eq!(
        many.validate(),
        Err(InvalidLookModel::BandCount {
            count: 17,
            max: MAX_LOOK_BANDS
        })
    );

    let outside = LookModelSettings {
        bands: vec![HueBandDefinition {
            id: band_id("red"),
            center_degrees: 360.0,
        }],
        ..default.clone()
    };
    assert!(matches!(
        outside.validate(),
        Err(InvalidLookModel::BandCenterOutOfRange { band_index: 0, .. })
    ));

    let close = |first: f32, second: f32, second_id: &str| LookModelSettings {
        bands: vec![
            HueBandDefinition {
                id: band_id("red"),
                center_degrees: first,
            },
            HueBandDefinition {
                id: band_id(second_id),
                center_degrees: second,
            },
        ],
        ..default.clone()
    };
    // Центры ближе зазора через границу круга и повтор идентификатора.
    assert!(matches!(
        close(0.2, 359.5, "blue").validate(),
        Err(InvalidLookModel::BandsTooClose { .. })
    ));
    assert!(matches!(
        close(10.0, 100.0, "red").validate(),
        Err(InvalidLookModel::BandsTooClose { .. })
    ));
    close(10.0, 100.0, "blue").validate().unwrap();

    let lut = LookModelSettings {
        max_lut_size: 1,
        ..default
    };
    assert_eq!(lut.validate(), Err(InvalidLookModel::LutLimit { value: 1, max: 129 }));
}
