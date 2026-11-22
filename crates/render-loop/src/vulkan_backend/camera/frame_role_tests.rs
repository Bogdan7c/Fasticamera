//! Роль кадра в видоискателе и его свет по результату кадра.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::flash::{FlashModeEcho, FlashResult, FlashShotId, FlashState, FrameLight};
use camera_contract::frame_passport::{FrameCaptureMeta, RawFramePassport};
use camera_contract::three_a::{BurstId, RequestOrigin};

use super::*;
use crate::vulkan_backend::camera::exposure::test_support::{UNIT_EXPOSURE, frame_at, missing_frame, new_stream};

fn frame(origin: RequestOrigin, flash: FlashResult) -> RawFramePassport {
    let mut passport = frame_at(&new_stream(), 1, Some(UNIT_EXPOSURE));
    if let FrameCaptureMeta::Paired(details) = &mut passport.capture {
        details.result.three_a.origin = origin;
        details.result.three_a.flash = flash;
    }
    passport
}

#[test]
fn burst_and_flash_still_frames_are_not_shown_everything_else_is() {
    let burst = RequestOrigin::Burst {
        burst: BurstId(1),
        index: 0,
    };
    let flash = RequestOrigin::FlashStill { shot: FlashShotId(5) };
    assert_eq!(
        viewfinder_frame_role(&frame(burst, FlashResult::UNREPORTED)),
        ViewfinderFrameRole::Bracket
    );
    assert_eq!(
        viewfinder_frame_role(&frame(flash, FlashResult::UNREPORTED)),
        ViewfinderFrameRole::FlashStill
    );
    for origin in [RequestOrigin::Repeating, RequestOrigin::Unknown] {
        assert!(viewfinder_frame_role(&frame(origin, FlashResult::UNREPORTED)).is_shown());
    }
    // Кадр без метаданных — обычный (его происхождение неизвестно).
    assert!(viewfinder_frame_role(&missing_frame(&new_stream(), 2)).is_shown());
}

#[test]
fn light_comes_from_the_result_of_the_frame_itself_and_missing_meta_is_ambient() {
    let torch = FlashResult {
        mode: Some(FlashModeEcho::Torch),
        state: Some(FlashState::Fired),
    };
    assert_eq!(
        frame_light(&frame(RequestOrigin::Repeating, torch)),
        FrameLight::TorchLit
    );
    assert_eq!(
        frame_light(&frame(RequestOrigin::Repeating, FlashResult::UNREPORTED)),
        FrameLight::Ambient
    );
    assert_eq!(frame_light(&missing_frame(&new_stream(), 2)), FrameLight::Ambient);
}
