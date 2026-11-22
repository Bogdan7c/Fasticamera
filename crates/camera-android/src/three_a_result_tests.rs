use camera_contract::flash::{FlashModeEcho, FlashResult, FlashShotId, FlashState};
use camera_contract::three_a::{BurstId, PointRequestId};

use super::*;
use crate::fake_metadata::{FakeMetadata, three_a_result_fixture};

fn parse(source: &FakeMetadata) -> ParsedThreeA {
    parse_three_a_result(source, RequestOrigin::Repeating)
}

#[test]
fn full_result_maps_every_key_and_keeps_the_origin() {
    let origin = RequestOrigin::Burst {
        burst: BurstId(2),
        index: 1,
    };
    let parsed = parse_three_a_result(&three_a_result_fixture(), origin);
    assert_eq!(
        parsed.state,
        ThreeAResultState {
            af_state: Some(AfState::FocusedLocked),
            ae_state: Some(AeState::Converged),
            lens_state: Some(LensState::Stationary),
            focus_distance_diopters: Some(2.5),
            scene_flicker: Some(SceneFlicker::Mains50Hz),
            flash: FlashResult {
                mode: Some(FlashModeEcho::Off),
                state: Some(FlashState::Ready),
            },
            origin,
        }
    );
    assert_eq!(parsed.unreadable_keys, Vec::new());
}

#[test]
fn every_af_state_code_maps_to_its_own_state() {
    let expected = [
        (tags::AF_STATE_INACTIVE, AfState::Inactive),
        (tags::AF_STATE_PASSIVE_SCAN, AfState::PassiveScan),
        (tags::AF_STATE_PASSIVE_FOCUSED, AfState::PassiveFocused),
        (tags::AF_STATE_ACTIVE_SCAN, AfState::ActiveScan),
        (tags::AF_STATE_FOCUSED_LOCKED, AfState::FocusedLocked),
        (tags::AF_STATE_NOT_FOCUSED_LOCKED, AfState::NotFocusedLocked),
        (tags::AF_STATE_PASSIVE_UNFOCUSED, AfState::PassiveUnfocused),
    ];
    for (code, state) in expected {
        let parsed = parse(&FakeMetadata::default().bytes(tags::AF_STATE, &[code]));
        assert_eq!(parsed.state.af_state, Some(state), "код {code}");
        assert_eq!(parsed.unreadable_keys, Vec::new(), "код {code}");
    }
}

#[test]
fn every_ae_lens_and_flicker_code_maps() {
    let ae = [
        (tags::AE_STATE_INACTIVE, AeState::Inactive),
        (tags::AE_STATE_SEARCHING, AeState::Searching),
        (tags::AE_STATE_CONVERGED, AeState::Converged),
        (tags::AE_STATE_LOCKED, AeState::Locked),
        (tags::AE_STATE_FLASH_REQUIRED, AeState::FlashRequired),
        (tags::AE_STATE_PRECAPTURE, AeState::Precapture),
    ];
    for (code, state) in ae {
        assert_eq!(
            parse(&FakeMetadata::default().bytes(tags::AE_STATE, &[code]))
                .state
                .ae_state,
            Some(state)
        );
    }
    let lens = [
        (tags::LENS_STATE_STATIONARY, LensState::Stationary),
        (tags::LENS_STATE_MOVING, LensState::Moving),
    ];
    for (code, state) in lens {
        assert_eq!(
            parse(&FakeMetadata::default().bytes(tags::LENS_STATE, &[code]))
                .state
                .lens_state,
            Some(state)
        );
    }
    let flicker = [
        (tags::SCENE_FLICKER_NONE, SceneFlicker::None),
        (tags::SCENE_FLICKER_50HZ, SceneFlicker::Mains50Hz),
        (tags::SCENE_FLICKER_60HZ, SceneFlicker::Mains60Hz),
    ];
    for (code, state) in flicker {
        assert_eq!(
            parse(&FakeMetadata::default().bytes(tags::SCENE_FLICKER, &[code]))
                .state
                .scene_flicker,
            Some(state)
        );
    }
}

#[test]
fn raw_only_session_without_three_a_keys_is_all_none_and_not_unreadable() {
    let origin = RequestOrigin::FocusTrigger {
        request: PointRequestId(4),
    };
    let parsed = parse_three_a_result(&FakeMetadata::default(), origin);
    assert_eq!(parsed.state, ThreeAResultState::unreported(origin));
    assert_eq!(parsed.unreadable_keys, Vec::new(), "отсутствие — не сбой чтения");
}

#[test]
fn unknown_enum_codes_become_none_and_are_listed_as_unreadable() {
    let source = three_a_result_fixture()
        .bytes(tags::AF_STATE, &[99])
        .bytes(tags::AE_STATE, &[99])
        .bytes(tags::LENS_STATE, &[99])
        .bytes(tags::SCENE_FLICKER, &[99]);
    let parsed = parse(&source);
    assert_eq!(parsed.state.af_state, None);
    assert_eq!(parsed.state.ae_state, None);
    assert_eq!(parsed.state.lens_state, None);
    assert_eq!(parsed.state.scene_flicker, None);
    assert_eq!(
        parsed.state.focus_distance_diopters,
        Some(2.5),
        "здоровый ключ не задет"
    );
    assert_eq!(
        parsed.unreadable_keys,
        vec![
            ThreeAResultKey::AfState,
            ThreeAResultKey::AeState,
            ThreeAResultKey::LensState,
            ThreeAResultKey::SceneFlicker
        ]
    );
}

#[test]
fn wrong_type_and_failed_read_are_unreadable_not_absent() {
    let source = three_a_result_fixture()
        .int32s(tags::AF_STATE, &[2])
        .unreadable(tags::LENS_FOCUS_DISTANCE, -10000);
    let parsed = parse(&source);
    assert_eq!(parsed.state.af_state, None);
    assert_eq!(parsed.state.focus_distance_diopters, None);
    assert_eq!(
        parsed.unreadable_keys,
        vec![ThreeAResultKey::AfState, ThreeAResultKey::FocusDistance]
    );
}

#[test]
fn negative_or_non_finite_focus_distance_is_unreadable() {
    for bad in [-0.5_f32, f32::NAN, f32::INFINITY] {
        let parsed = parse(&three_a_result_fixture().floats(tags::LENS_FOCUS_DISTANCE, &[bad]));
        assert_eq!(parsed.state.focus_distance_diopters, None, "{bad}");
        assert_eq!(parsed.unreadable_keys, vec![ThreeAResultKey::FocusDistance], "{bad}");
    }
    let infinity_focus = parse(&three_a_result_fixture().floats(tags::LENS_FOCUS_DISTANCE, &[0.0]));
    assert_eq!(
        infinity_focus.state.focus_distance_diopters,
        Some(0.0),
        "0 диоптрий — бесконечность, это значение"
    );
}

#[test]
fn every_flash_mode_and_state_code_maps_to_its_own_value() {
    let modes = [
        (tags::FLASH_MODE_OFF, FlashModeEcho::Off),
        (tags::FLASH_MODE_SINGLE, FlashModeEcho::Single),
        (tags::FLASH_MODE_TORCH, FlashModeEcho::Torch),
    ];
    for (code, mode) in modes {
        let parsed = parse(&FakeMetadata::default().bytes(tags::FLASH_MODE, &[code]));
        assert_eq!(parsed.state.flash.mode, Some(mode), "код {code}");
        assert_eq!(parsed.state.flash.state, None, "код {code}");
        assert_eq!(parsed.unreadable_keys, Vec::new(), "код {code}");
    }
    let states = [
        (tags::FLASH_STATE_UNAVAILABLE, FlashState::Unavailable),
        (tags::FLASH_STATE_CHARGING, FlashState::Charging),
        (tags::FLASH_STATE_READY, FlashState::Ready),
        (tags::FLASH_STATE_FIRED, FlashState::Fired),
        (tags::FLASH_STATE_PARTIAL, FlashState::Partial),
    ];
    for (code, state) in states {
        let parsed = parse(&FakeMetadata::default().bytes(tags::FLASH_STATE, &[code]));
        assert_eq!(parsed.state.flash.state, Some(state), "код {code}");
        assert_eq!(parsed.state.flash.mode, None, "код {code}");
        assert_eq!(parsed.unreadable_keys, Vec::new(), "код {code}");
    }
}

#[test]
fn torch_and_single_frames_are_told_apart_by_their_own_result() {
    let torch = parse(
        &FakeMetadata::default()
            .bytes(tags::FLASH_MODE, &[tags::FLASH_MODE_TORCH])
            .bytes(tags::FLASH_STATE, &[tags::FLASH_STATE_FIRED]),
    );
    assert_eq!(torch.state.flash.light(), camera_contract::flash::FrameLight::TorchLit);
    let single = parse_three_a_result(
        &FakeMetadata::default()
            .bytes(tags::FLASH_MODE, &[tags::FLASH_MODE_SINGLE])
            .bytes(tags::FLASH_STATE, &[tags::FLASH_STATE_FIRED]),
        RequestOrigin::FlashStill { shot: FlashShotId(5) },
    );
    assert_eq!(
        single.state.flash.light(),
        camera_contract::flash::FrameLight::FlashFired
    );
    assert_eq!(single.state.origin, RequestOrigin::FlashStill { shot: FlashShotId(5) });
}

#[test]
fn absent_flash_keys_are_unreported_and_not_unreadable() {
    let parsed = parse(
        &three_a_result_fixture()
            .without(tags::FLASH_MODE)
            .without(tags::FLASH_STATE),
    );
    assert_eq!(parsed.state.flash, FlashResult::UNREPORTED);
    assert_eq!(parsed.unreadable_keys, Vec::new());
}

#[test]
fn unknown_or_wrong_type_flash_values_are_unreadable_and_leave_the_other_key_intact() {
    let unknown = parse(
        &three_a_result_fixture()
            .bytes(tags::FLASH_MODE, &[99])
            .bytes(tags::FLASH_STATE, &[99]),
    );
    assert_eq!(unknown.state.flash, FlashResult::UNREPORTED);
    assert_eq!(
        unknown.unreadable_keys,
        vec![ThreeAResultKey::FlashMode, ThreeAResultKey::FlashState]
    );
    let half = parse(&three_a_result_fixture().int32s(tags::FLASH_STATE, &[3]));
    assert_eq!(
        half.state.flash.mode,
        Some(FlashModeEcho::Off),
        "здоровый ключ не задет"
    );
    assert_eq!(half.state.flash.state, None);
    assert_eq!(half.unreadable_keys, vec![ThreeAResultKey::FlashState]);
}
