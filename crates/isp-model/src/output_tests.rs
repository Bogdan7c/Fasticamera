//! Тесты профиля выхода: конструкторы, отказ заделов и выбор кривой.

use super::{OutputBitDepth, OutputProfile, OutputRendering, OutputTransfer, UnsupportedOutputProfile};
use crate::image::DisplayGamut;
use crate::transfer::{srgb_decode, srgb_encode};

#[test]
fn srgb_and_display_p3_profiles_are_supported_sdr_eight_bit() {
    for (profile, gamut) in [
        (OutputProfile::srgb(), DisplayGamut::Srgb),
        (OutputProfile::display_p3(), DisplayGamut::DisplayP3),
        (OutputProfile::sdr(DisplayGamut::DisplayP3), DisplayGamut::DisplayP3),
    ] {
        assert_eq!(profile.gamut(), gamut);
        assert_eq!(profile.rendering(), OutputRendering::DisplayReferredSdr);
        assert_eq!(profile.transfer(), OutputTransfer::Srgb);
        assert_eq!(profile.bit_depth(), OutputBitDepth::Eight);
        assert_eq!(profile.check_supported(), Ok(()));
    }
}

#[test]
fn log_and_hdr_renderings_are_refused_with_distinct_errors() {
    for rendering in [OutputRendering::SceneLog, OutputRendering::Hdr] {
        let profile = OutputProfile::display_p3().with_rendering(rendering);
        assert_eq!(
            profile.check_supported(),
            Err(UnsupportedOutputProfile::Rendering(rendering))
        );
        // Охват профиля при отказе не теряется: ошибка про рендеринг, а не про охват.
        assert_eq!(profile.gamut(), DisplayGamut::DisplayP3);
    }
}

#[test]
fn ten_bit_depth_is_refused_as_its_own_error() {
    let profile = OutputProfile::srgb().with_bit_depth(OutputBitDepth::Ten);
    assert_eq!(
        profile.check_supported(),
        Err(UnsupportedOutputProfile::BitDepth(OutputBitDepth::Ten))
    );
}

#[test]
fn rendering_error_wins_over_bit_depth_when_both_are_unsupported() {
    let profile = OutputProfile::srgb()
        .with_rendering(OutputRendering::Hdr)
        .with_bit_depth(OutputBitDepth::Ten);
    assert_eq!(
        profile.check_supported(),
        Err(UnsupportedOutputProfile::Rendering(OutputRendering::Hdr))
    );
}

#[test]
fn srgb_transfer_matches_the_srgb_curve_bit_for_bit() {
    for step in 0..=256_u32 {
        let value = step as f32 / 256.0;
        assert_eq!(OutputTransfer::Srgb.encode(value), srgb_encode(value));
        assert_eq!(OutputTransfer::Srgb.decode(value), srgb_decode(value));
    }
    assert_eq!(OutputTransfer::Srgb.encode(f32::NAN), 0.0);
}
