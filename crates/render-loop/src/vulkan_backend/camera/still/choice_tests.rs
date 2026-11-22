//! Тесты перевода итога кольца в словарь снимка на настоящем кольце: точное попадание, каждая причина замены,
//! каждая причина «кадра нет». Лизинги возвращаются явно (утечек нет).

use camera_contract::three_a::{BurstId, RequestOrigin};
use raw_frame_stream::{RawFrameStreamConfig, RingPickRequest};
use still_contract::frame_choice::{FrameChoice, NoFrameReason, SubstitutionReason};

use super::*;
use crate::vulkan_backend::camera::still::test_support::{MAIN, StreamRig, ms, stamp};

fn frame_choice(translated: TranslatedPick) -> (FrameChoice, PickDetail) {
    match translated {
        TranslatedPick::Frame { lease, choice, detail } => {
            lease.release_unused();
            (choice, detail)
        }
        TranslatedPick::NoFrame { reason, .. } => panic!("ожидали кадр, получили отказ {reason:?}"),
    }
}

fn no_frame(translated: TranslatedPick) -> (NoFrameReason, PickDetail) {
    match translated {
        TranslatedPick::NoFrame { reason, detail } => (reason, detail),
        TranslatedPick::Frame { lease, .. } => {
            lease.release_unused();
            panic!("ожидали отказ, получили кадр");
        }
    }
}

#[test]
fn shown_frame_in_the_ring_is_an_exact_choice_with_its_shown_time() {
    let mut rig = StreamRig::with_ring(MAIN);
    let frames = rig.offer_many(3);
    let pick = rig.stream.snapshot_ring().pick(RingPickRequest::ShownFrame(frames[1]));
    let (choice, detail) = frame_choice(translate_pick(pick, ShownAt::Proven(ms(40))));
    assert_eq!(
        choice,
        FrameChoice::ShownAtPress {
            sensor_timestamp: frames[1],
            shown_at: ms(40)
        }
    );
    assert_eq!(detail, PickDetail::Direct);
}

#[test]
fn estimate_request_is_a_substitution_with_unknown_timing_and_names_the_wanted_frame() {
    let mut rig = StreamRig::with_ring(MAIN);
    let frames = rig.offer_many(3);
    let pick = rig
        .stream
        .snapshot_ring()
        .pick(RingPickRequest::NearestToEstimate(frames[2]));
    let (choice, _) = frame_choice(translate_pick(pick, ShownAt::Unknown));
    assert_eq!(
        choice,
        FrameChoice::Substituted {
            sensor_timestamp: frames[2],
            wanted: Some(frames[2]),
            reason: SubstitutionReason::PresentationTimingUnknown
        }
    );
}

#[test]
fn an_evicted_frame_is_replaced_by_the_nearest_with_the_evicted_reason() {
    let mut rig = StreamRig::new(
        RawFrameStreamConfig {
            snapshot_ring_max_frames: 3,
            ..RawFrameStreamConfig::default()
        },
        MAIN,
    );
    let frames = rig.offer_many(6);
    // Кадр 0 вытеснен из кольца в 3 кадра.
    let pick = rig.stream.snapshot_ring().pick(RingPickRequest::ShownFrame(frames[0]));
    let (choice, detail) = frame_choice(translate_pick(pick, ShownAt::Proven(ms(1))));
    assert_eq!(detail, PickDetail::Direct);
    match choice {
        FrameChoice::Substituted {
            sensor_timestamp,
            wanted,
            reason,
        } => {
            assert_eq!(reason, SubstitutionReason::ShownFrameEvicted);
            assert_eq!(wanted, Some(frames[0]));
            assert_eq!(sensor_timestamp, frames[3], "ближайший оставшийся в кольце");
        }
        other => panic!("ожидали замену, получили {other:?}"),
    }
}

#[test]
fn a_burst_frame_is_not_regular_and_is_replaced() {
    let mut rig = StreamRig::with_ring(MAIN);
    let before = rig.offer();
    let burst = rig.offer_with_origin(
        RequestOrigin::Burst {
            burst: BurstId(1),
            index: 0,
        },
        true,
    );
    let pick = rig.stream.snapshot_ring().pick(RingPickRequest::ShownFrame(burst));
    let (choice, _) = frame_choice(translate_pick(pick, ShownAt::Proven(ms(1))));
    assert_eq!(
        choice,
        FrameChoice::Substituted {
            sensor_timestamp: before,
            wanted: Some(burst),
            reason: SubstitutionReason::ShownFrameNotRegular
        }
    );
}

#[test]
fn a_frame_without_metadata_is_replaced_with_the_metadata_reason() {
    let mut rig = StreamRig::with_ring(MAIN);
    let before = rig.offer();
    let bare = rig.offer_with_origin(RequestOrigin::Repeating, false);
    // Кадр без пары уходит в кольцо после срока ожидания пары: даём ему «созреть» новыми кадрами.
    rig.offer_many(3);
    let pick = rig.stream.snapshot_ring().pick(RingPickRequest::ShownFrame(bare));
    let (choice, _) = frame_choice(translate_pick(pick, ShownAt::Proven(ms(1))));
    let FrameChoice::Substituted {
        reason,
        wanted,
        sensor_timestamp,
    } = choice
    else {
        panic!("ожидали замену");
    };
    assert_eq!(wanted, Some(bare));
    assert_ne!(sensor_timestamp, bare);
    assert!(
        matches!(
            reason,
            SubstitutionReason::ShownFrameWithoutMetadata | SubstitutionReason::ShownFrameEvicted
        ),
        "{reason:?} (кадр до него: {before:?})"
    );
}

#[test]
fn a_target_that_never_reached_the_ring_is_counted_apart_from_eviction() {
    let mut rig = StreamRig::with_ring(MAIN);
    let frames = rig.offer_many(3);
    // Метка между кадрами кольца: не старше старейшего, но кадра с такой меткой нет.
    let missing = stamp(frames[1].as_nanos() + 1);
    let pick = rig.stream.snapshot_ring().pick(RingPickRequest::ShownFrame(missing));
    let (choice, detail) = frame_choice(translate_pick(pick, ShownAt::Proven(ms(1))));
    assert_eq!(detail, PickDetail::TargetLostBeforeRing);
    assert!(matches!(
        choice,
        FrameChoice::Substituted {
            reason: SubstitutionReason::ShownFrameEvicted,
            ..
        }
    ));
}

#[test]
fn unavailable_reasons_map_to_no_frame_reasons() {
    let disabled = StreamRig::new(
        RawFrameStreamConfig {
            snapshot_ring_max_frames: 0,
            ..RawFrameStreamConfig::default()
        },
        MAIN,
    );
    let pick = disabled
        .stream
        .snapshot_ring()
        .pick(RingPickRequest::NearestToEstimate(stamp(1)));
    assert_eq!(
        no_frame(translate_pick(pick, ShownAt::Unknown)).0,
        NoFrameReason::RingDisabled
    );

    let empty = StreamRig::with_ring(MAIN);
    let pick = empty
        .stream
        .snapshot_ring()
        .pick(RingPickRequest::NearestToEstimate(stamp(1)));
    assert_eq!(
        no_frame(translate_pick(pick, ShownAt::Unknown)),
        (NoFrameReason::RingEmpty, PickDetail::Direct)
    );

    // Кадры есть, но все серии: годных нет.
    let mut only_bursts = StreamRig::with_ring(MAIN);
    only_bursts.offer_with_origin(
        RequestOrigin::Burst {
            burst: BurstId(1),
            index: 0,
        },
        true,
    );
    let pick = only_bursts
        .stream
        .snapshot_ring()
        .pick(RingPickRequest::NearestToEstimate(stamp(1)));
    assert_eq!(
        no_frame(translate_pick(pick, ShownAt::Unknown)),
        (NoFrameReason::RingEmpty, PickDetail::NoEligibleFrame)
    );
}
