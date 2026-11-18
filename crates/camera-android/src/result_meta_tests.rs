use super::*;
use crate::fake_metadata::FakeMetadata;
use camera_contract::capabilities::Rational;

/// Результат кадра основной камеры в режиме CamX 4:3@60 (00 C2): метка сенсора, 8 мс, ISO 800,
/// ЛОЖНАЯ длительность кадра 33,3 мс, rolling shutter skew 13,66 мс.
fn camx_frame_result() -> FakeMetadata {
    FakeMetadata::default()
        .int64s(tags::SENSOR_TIMESTAMP, &[1_234_567_890_123])
        .int64s(tags::SENSOR_EXPOSURE_TIME, &[8_000_000])
        .int32s(tags::SENSOR_SENSITIVITY, &[800])
        .int64s(tags::SENSOR_FRAME_DURATION, &[33_333_333])
        .int64s(tags::ROLLING_SHUTTER_SKEW, &[13_660_000])
}

#[test]
fn full_result_carries_every_number_and_the_given_frame_number() {
    let parsed = parse_capture_result(&camx_frame_result(), FrameNumber(42), RequestOrigin::Repeating);
    assert_eq!(parsed.unreadable_keys, Vec::new());
    assert_eq!(parsed.meta.frame_number, FrameNumber(42));
    assert_eq!(
        parsed.meta.sensor_timestamp,
        Some(SensorTimestamp::from_nanos(1_234_567_890_123))
    );
    assert_eq!(parsed.meta.exposure_time, Some(Duration::from_millis(8)));
    assert_eq!(parsed.meta.sensitivity_iso, Some(800));
    // Длительность кадра передаётся как есть: что она ложная, знает потребитель (контракт, 00 C2).
    assert_eq!(
        parsed.meta.reported_frame_duration,
        Some(FrameDuration::from_nanos(33_333_333))
    );
    assert_eq!(parsed.meta.rolling_shutter_skew, Some(Duration::from_micros(13_660)));
    assert_eq!(parsed.meta.missing_keys(), Vec::new());
}

#[test]
fn keys_hal_did_not_send_stay_none_and_are_not_reported_as_unreadable() {
    let source = camx_frame_result()
        .without(tags::ROLLING_SHUTTER_SKEW)
        .without(tags::SENSOR_SENSITIVITY);
    let parsed = parse_capture_result(&source, FrameNumber(1), RequestOrigin::Repeating);
    assert_eq!(parsed.meta.rolling_shutter_skew, None);
    assert_eq!(parsed.meta.sensitivity_iso, None);
    assert_eq!(parsed.unreadable_keys, Vec::new(), "отсутствие — не сбой чтения");
    assert_eq!(
        parsed.meta.missing_keys(),
        vec![ResultKey::Sensitivity, ResultKey::RollingShutterSkew]
    );
}

#[test]
fn present_but_unusable_keys_are_listed_separately_in_key_order() {
    let source = camx_frame_result()
        .int64s(tags::SENSOR_EXPOSURE_TIME, &[-5])
        .floats(tags::SENSOR_SENSITIVITY, &[800.0])
        .unreadable(tags::ROLLING_SHUTTER_SKEW, -10000);
    let parsed = parse_capture_result(&source, FrameNumber(1), RequestOrigin::Repeating);
    assert_eq!(parsed.meta.exposure_time, None);
    assert_eq!(parsed.meta.sensitivity_iso, None);
    assert_eq!(parsed.meta.rolling_shutter_skew, None);
    assert_eq!(
        parsed.unreadable_keys,
        vec![
            ResultKey::ExposureTime,
            ResultKey::Sensitivity,
            ResultKey::RollingShutterSkew
        ]
    );
    assert!(parsed.meta.sensor_timestamp.is_some(), "годные ключи не страдают");
}

#[test]
fn sensor_timestamp_reader_distinguishes_absent_from_present() {
    assert_eq!(
        read_sensor_timestamp(&camx_frame_result()),
        Some(SensorTimestamp::from_nanos(1_234_567_890_123))
    );
    assert_eq!(read_sensor_timestamp(&FakeMetadata::default()), None);
}

fn full_color_result() -> FakeMetadata {
    camx_frame_result()
        .floats(tags::DYNAMIC_BLACK_LEVEL, &[64.0, 64.5, 65.0, 64.25])
        .int32s(tags::DYNAMIC_WHITE_LEVEL, &[1023])
        .floats(tags::COLOR_CORRECTION_GAINS, &[2.0, 1.0, 1.0, 1.5])
        .with(
            tags::NEUTRAL_COLOR_POINT,
            crate::entry::EntryValue::Rationals(vec![
                Rational {
                    numerator: 1,
                    denominator: 2,
                },
                Rational {
                    numerator: 1,
                    denominator: 1,
                },
                Rational {
                    numerator: 3,
                    denominator: 4,
                },
            ]),
        )
}

#[test]
fn color_hints_are_read_key_by_key_into_the_passport_shape() {
    let parsed = parse_capture_result(&full_color_result(), FrameNumber(1), RequestOrigin::Repeating);
    assert_eq!(parsed.unreadable_color_keys, Vec::new());
    assert_eq!(parsed.color.dynamic_black_level, Some([64.0, 64.5, 65.0, 64.25]));
    assert_eq!(parsed.color.dynamic_white_level, Some(1023));
    assert_eq!(
        parsed.color.color_gains,
        Some(ColorCorrectionGains([2.0, 1.0, 1.0, 1.5]))
    );
    let neutral = parsed.color.neutral_color_point.expect("нейтральная точка");
    assert_eq!((neutral[0].numerator, neutral[0].denominator), (1, 2));
    assert_eq!((neutral[2].numerator, neutral[2].denominator), (3, 4));
}

#[test]
fn color_keys_hal_did_not_send_are_none_and_not_unreadable() {
    // OnePlus 11 не отдаёт динамический black level (00 C3): отсутствие — не сбой чтения.
    let source = full_color_result().without(tags::DYNAMIC_BLACK_LEVEL);
    let parsed = parse_capture_result(&source, FrameNumber(1), RequestOrigin::Repeating);
    assert_eq!(parsed.color.dynamic_black_level, None);
    assert!(parsed.color.color_gains.is_some(), "соседние ключи не страдают");
    assert_eq!(parsed.unreadable_color_keys, Vec::new());
    let bare = parse_capture_result(&camx_frame_result(), FrameNumber(1), RequestOrigin::Repeating);
    assert_eq!(bare.color, CaptureColorHints::default());
}

#[test]
fn malformed_color_keys_become_none_and_are_listed_in_order() {
    let source = full_color_result()
        // слишком короткий массив усилений
        .floats(tags::COLOR_CORRECTION_GAINS, &[2.0, 1.0])
        // не тот тип
        .int32s(tags::DYNAMIC_BLACK_LEVEL, &[64])
        // отрицательный белый уровень
        .int32s(tags::DYNAMIC_WHITE_LEVEL, &[-1])
        .unreadable(tags::NEUTRAL_COLOR_POINT, -10000);
    let parsed = parse_capture_result(&source, FrameNumber(1), RequestOrigin::Repeating);
    assert_eq!(parsed.color, CaptureColorHints::default());
    assert_eq!(
        parsed.unreadable_color_keys,
        vec![
            ColorHintKey::DynamicBlackLevel,
            ColorHintKey::DynamicWhiteLevel,
            ColorHintKey::ColorCorrectionGains,
            ColorHintKey::NeutralColorPoint
        ]
    );
    assert_eq!(parsed.unreadable_keys, Vec::new(), "основные ключи результата целы");
}

#[test]
fn three_a_state_and_origin_travel_with_the_result_and_unreadable_keys_are_reported() {
    use camera_contract::three_a::{AfState, BurstId, PointRequestId};
    let origin = RequestOrigin::FocusTrigger {
        request: PointRequestId(7),
    };
    let source = camx_frame_result()
        .bytes(tags::AF_STATE, &[tags::AF_STATE_FOCUSED_LOCKED])
        .bytes(tags::AE_STATE, &[200]);
    let parsed = parse_capture_result(&source, FrameNumber(5), origin);
    assert_eq!(parsed.meta.three_a.origin, origin);
    assert_eq!(parsed.meta.three_a.af_state, Some(AfState::FocusedLocked));
    assert_eq!(parsed.meta.three_a.ae_state, None);
    assert_eq!(
        parsed.unreadable_three_a_keys,
        vec![crate::three_a_result::ThreeAResultKey::AeState]
    );
    assert_eq!(
        parsed.unreadable_keys,
        Vec::new(),
        "3A не смешивается с прежними ключами"
    );

    let burst = RequestOrigin::Burst {
        burst: BurstId(1),
        index: 2,
    };
    assert_eq!(
        parse_capture_result(&camx_frame_result(), FrameNumber(6), burst)
            .meta
            .three_a
            .origin,
        burst
    );
}
