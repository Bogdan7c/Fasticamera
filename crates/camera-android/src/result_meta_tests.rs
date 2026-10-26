use super::*;
use crate::fake_metadata::FakeMetadata;

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
    let parsed = parse_capture_result(&camx_frame_result(), FrameNumber(42));
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
    let parsed = parse_capture_result(&source, FrameNumber(1));
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
    let parsed = parse_capture_result(&source, FrameNumber(1));
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
