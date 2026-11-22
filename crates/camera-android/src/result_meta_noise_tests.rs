//! Профиль шума из результата камеры: разбор, отсутствие и нечитаемые записи.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тесты")]

use camera_contract::capture_result::FrameNumber;
use camera_contract::frame_passport::{NoiseModelCoefficients, NoiseProfile};
use camera_contract::three_a::RequestOrigin;

use super::{ColorHintKey, parse_capture_result};
use crate::entry::EntryValue;
use crate::fake_metadata::FakeMetadata;
use crate::tags;

fn parse(source: &FakeMetadata) -> super::ParsedCaptureResult {
    parse_capture_result(source, FrameNumber(1), RequestOrigin::Repeating)
}

fn with_doubles(values: &[f64]) -> FakeMetadata {
    FakeMetadata::default().with(tags::NOISE_PROFILE, EntryValue::Doubles(values.to_vec()))
}

#[test]
fn noise_profile_pairs_become_scale_and_offset_per_cfa_channel() {
    let parsed = parse(&with_doubles(&[
        1.0e-3, 2.0e-6, 1.1e-3, 2.1e-6, 1.2e-3, 2.2e-6, 1.3e-3, 2.3e-6,
    ]));

    assert_eq!(parsed.unreadable_color_keys, Vec::new());
    let channel = |scale, offset| NoiseModelCoefficients { scale, offset };
    assert_eq!(
        parsed.color.noise_profile,
        Some(NoiseProfile([
            channel(1.0e-3, 2.0e-6),
            channel(1.1e-3, 2.1e-6),
            channel(1.2e-3, 2.2e-6),
            channel(1.3e-3, 2.3e-6),
        ]))
    );
}

#[test]
fn absent_noise_profile_is_none_without_a_complaint() {
    let parsed = parse(&FakeMetadata::default());
    assert_eq!(parsed.color.noise_profile, None);
    assert_eq!(parsed.unreadable_color_keys, Vec::new());
}

#[test]
fn malformed_noise_profiles_become_none_and_are_listed_as_unreadable() {
    let nan = [1.0e-3, 2.0e-6, f64::NAN, 2.1e-6, 1.2e-3, 2.2e-6, 1.3e-3, 2.3e-6];
    let negative_scale = [1.0e-3, 2.0e-6, -1.1e-3, 2.1e-6, 1.2e-3, 2.2e-6, 1.3e-3, 2.3e-6];
    let infinite_offset = [1.0e-3, f64::INFINITY, 1.1e-3, 2.1e-6, 1.2e-3, 2.2e-6, 1.3e-3, 2.3e-6];
    let broken: [(&str, FakeMetadata); 6] = [
        ("NaN", with_doubles(&nan)),
        ("отрицательный S", with_doubles(&negative_scale)),
        ("бесконечный O", with_doubles(&infinite_offset)),
        (
            "6 чисел",
            with_doubles(&[1.0e-3, 2.0e-6, 1.1e-3, 2.1e-6, 1.2e-3, 2.2e-6]),
        ),
        ("10 чисел", with_doubles(&[1.0e-3; 10])),
        (
            "тип float вместо double",
            FakeMetadata::default().floats(tags::NOISE_PROFILE, &[1.0e-3; 8]),
        ),
    ];
    for (case, source) in broken {
        let parsed = parse(&source);
        assert_eq!(parsed.color.noise_profile, None, "{case}");
        assert_eq!(parsed.unreadable_color_keys, vec![ColorHintKey::NoiseProfile], "{case}");
        assert_eq!(parsed.unreadable_keys, Vec::new(), "{case}: основные ключи целы");
    }
    let unreadable = parse(&FakeMetadata::default().unreadable(tags::NOISE_PROFILE, -10000));
    assert_eq!(unreadable.unreadable_color_keys, vec![ColorHintKey::NoiseProfile]);
}

#[test]
fn zero_scale_is_valid_and_a_bad_noise_profile_does_not_spoil_neighbouring_hints() {
    let ok = parse(&with_doubles(&[0.0, 1.0e-6, 0.0, 1.0e-6, 0.0, 1.0e-6, 0.0, -1.0e-9]));
    assert_eq!(ok.unreadable_color_keys, Vec::new());
    assert_eq!(ok.color.noise_profile.expect("профиль").0[3].offset, -1.0e-9);

    let source = with_doubles(&[f64::NAN; 8])
        .floats(tags::COLOR_CORRECTION_GAINS, &[2.0, 1.0, 1.0, 1.5])
        .int32s(tags::DYNAMIC_WHITE_LEVEL, &[-1]);
    let parsed = parse(&source);
    assert_eq!(
        parsed.unreadable_color_keys,
        vec![ColorHintKey::DynamicWhiteLevel, ColorHintKey::NoiseProfile],
        "порядок как в ColorHintKey"
    );
    assert!(parsed.color.color_gains.is_some());
}
