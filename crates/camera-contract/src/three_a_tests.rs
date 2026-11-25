//! Тесты словаря 3A: проверка значений на входе, ящик указаний, серия.

use std::time::Duration;

use super::*;
use crate::control::{ExposureTime, Iso};

#[test]
fn sensor_point_and_region_reject_values_outside_the_frame() {
    assert!(SensorPoint::new(0.0, 1.0).is_ok(), "края кадра допустимы");
    assert_eq!(
        SensorPoint::new(1.2, 0.5),
        Err(InvalidMeteringRegion::CoordinateOutOfRange { value: 1.2 })
    );
    assert!(
        SensorPoint::new(f32::NAN, 0.5).is_err(),
        "NaN не проходит как координата"
    );
    assert!(MeteringRegion::new(SensorPoint::CENTER, 0.0).is_err());
    assert!(MeteringRegion::new(SensorPoint::CENTER, f32::NAN).is_err());
    assert!(MeteringRegion::new(SensorPoint::CENTER, 1.0).is_ok());
}

#[test]
fn directive_slot_hands_the_reader_only_newer_directives() {
    let writer: DirectiveSlot<ThreeAProfile> = DirectiveSlot::new();
    let reader = writer.clone();
    assert_eq!(
        reader.latest(),
        None,
        "пока ничего не публиковали — пусто, а не выдумка"
    );

    let first = writer.publish(ThreeAProfile::Photo);
    let video = ThreeAProfile::Video;
    let second = writer.publish(video);
    assert!(second > first);

    // Читатель, видевший первое указание, получает только второе — и ровно его значение.
    assert_eq!(reader.newer_than(Some(first)), Some((second, video)));
    assert_eq!(
        reader.newer_than(Some(second)),
        None,
        "повтор уже применённого не отдаётся"
    );
    assert_eq!(reader.newer_than(None), Some((second, video)));
}

#[test]
fn bracket_rejects_empty_and_oversized_series() {
    let frame = BracketExposure {
        exposure_time: ExposureTime::new(Duration::from_millis(2)).unwrap(),
        iso: Iso::new(400).unwrap(),
    };
    assert_eq!(ExposureBracket::new(Vec::new(), 8), Err(InvalidBracket::Empty));
    assert_eq!(
        ExposureBracket::new(vec![frame; 9], 8),
        Err(InvalidBracket::TooManyFrames { frames: 9, limit: 8 })
    );
    let bracket = ExposureBracket::new(vec![frame; 4], 8).unwrap();
    assert_eq!(bracket.frames().len(), 4);
}

#[test]
fn mains_frequency_reads_from_config_strings() {
    let parsed: MainsFrequency = serde_json::from_str("\"50hz\"").unwrap();
    assert_eq!(parsed, MainsFrequency::Hz50);
    assert!(serde_json::from_str::<MainsFrequency>("\"55hz\"").is_err());
}
