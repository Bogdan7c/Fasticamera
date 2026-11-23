//! Тесты фактов кадра снимка: кадрирование из ПЛАНА, нейтраль DNG, подтяжка, поворот 4×2, отказы.

use std::sync::Arc;

use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::{FrameCaptureMeta, MissingCaptureMeta, RawFramePassport};
use camera_contract::geometry::PixelSize;
use camera_contract::viewfinder_crop::CropAspect;
use isp_model::color::chromaticity::Chromaticity;
use isp_model::exposure::ExposureGain;
use isp_model::lens_shading::{LensShadingGainMap, LensShadingSkipReason, LensShadingStep};
use shell_contract::orientation::DeviceOrientation;
use still_contract::frame_choice::{FrameChoice, SubstitutionReason};
use still_contract::shot::StillRotation;

use super::*;
use crate::vulkan_backend::camera::still::test_support::{FRONT, MAIN, StreamRig, framing, plan, stamp};

fn passport_of(rig: &mut StreamRig) -> RawFramePassport {
    let ts = rig.offer();
    let lease = rig
        .stream
        .snapshot_ring()
        .take_all()
        .into_iter()
        .find(|lease| lease.passport().sensor_timestamp == ts);
    let lease = lease.expect("кадр в кольце");
    let passport = lease.passport().clone();
    lease.release_unused();
    passport
}

fn choice(passport: &RawFramePassport) -> FrameChoice {
    FrameChoice::Substituted {
        sensor_timestamp: passport.sensor_timestamp,
        wanted: None,
        reason: SubstitutionReason::PresentationTimingUnknown,
    }
}

fn params(passport: &RawFramePassport, gain_ev: f32) -> FrameStillParams {
    FrameStillParams {
        stream: Arc::clone(&passport.stream),
        white: Chromaticity { x: 0.3127, y: 0.3290 },
        gain: ExposureGain::from_ev(gain_ev).expect("подтяжка"),
    }
}

/// Виньетирование не применялось: тесты, не про него, берут этот ответ.
fn not_applied() -> AppliedLensShading {
    AppliedLensShading::NotApplied(LensShadingNotAppliedReason::NotCalibrated)
}

#[test]
fn facts_carry_the_plan_crop_the_frame_own_gain_and_a_unit_maximum_neutral() {
    let mut rig = StreamRig::with_ring(MAIN);
    let passport = passport_of(&mut rig);
    let mut still = plan(5, 100);
    // План просит 16:9 при зуме 2×: кадрирование берётся из плана, а не откуда-то ещё.
    still.framing = framing(CropAspect::LongToShort { long: 16, short: 9 }, 2.0);
    let facts = build_facts(
        &still,
        &passport,
        choice(&passport),
        &params(&passport, 0.75),
        not_applied(),
    )
    .expect("факты");
    assert_eq!(facts.id, still.id);
    assert_eq!(facts.sensor_timestamp, passport.sensor_timestamp);
    let expected_crop = still
        .framing
        .crop_for_camera(&camera_id(MAIN))
        .crop
        .resolve(passport.stream.valid_area)
        .expect("кадрирование");
    assert_eq!(facts.crop, expected_crop);
    assert!(
        facts.crop.size.width < passport.stream.valid_area.size.width,
        "зум 2× меньше кадра"
    );
    assert!(
        (facts.digital_gain.0 - 0.75).abs() < 1e-6,
        "DigitalGainEv = log2 k кадра"
    );
    let maximum = facts.neutral.0.iter().copied().fold(f64::MIN, f64::max);
    assert!(
        (maximum - 1.0).abs() < 1e-12,
        "нейтраль нормирована на максимум 1: {:?}",
        facts.neutral
    );
    assert!(facts.neutral.0.iter().all(|channel| *channel > 0.0));
}

#[test]
fn neutral_comes_from_our_white_and_the_stream_profile_not_from_hal_hints() {
    let mut rig = StreamRig::with_ring(MAIN);
    let passport = passport_of(&mut rig);
    let still = plan(1, 1);
    let warm = FrameStillParams {
        white: Chromaticity { x: 0.44, y: 0.40 },
        ..params(&passport, 0.0)
    };
    let cold = FrameStillParams {
        white: Chromaticity { x: 0.28, y: 0.29 },
        ..params(&passport, 0.0)
    };
    let warm_facts = build_facts(&still, &passport, choice(&passport), &warm, not_applied()).expect("факты");
    let cold_facts = build_facts(&still, &passport, choice(&passport), &cold, not_applied()).expect("факты");
    assert_ne!(
        warm_facts.neutral, cold_facts.neutral,
        "нейтраль следует за нашей белой точкой"
    );
}

#[test]
fn rotation_covers_four_orientations_on_both_sides() {
    let table_back = [
        (DeviceOrientation::Portrait, StillRotation::Clockwise90),
        (DeviceOrientation::LandscapeRight, StillRotation::Clockwise180),
        (DeviceOrientation::PortraitUpsideDown, StillRotation::Clockwise270),
        (DeviceOrientation::LandscapeLeft, StillRotation::None),
    ];
    let mut back = StreamRig::with_ring(MAIN);
    let back_passport = passport_of(&mut back);
    for (orientation, expected) in table_back {
        let mut still = plan(1, 1);
        still.orientation = orientation;
        let facts = build_facts(
            &still,
            &back_passport,
            choice(&back_passport),
            &params(&back_passport, 0.0),
            not_applied(),
        )
        .expect("факты");
        assert_eq!(facts.rotation, expected, "задняя, {orientation:?}");
    }

    // Фронтальная: сенсор 270° (как у OnePlus 11), поворот телефона вычитается.
    let mut front_passport_value = super::super::test_support::stream_passport(FRONT);
    front_passport_value.sensor_orientation = camera_contract::geometry::SensorOrientation::Degrees270;
    let front_stream = Arc::new(front_passport_value);
    let front_passport = RawFramePassport {
        stream: Arc::clone(&front_stream),
        ..back_passport.clone()
    };
    let table_front = [
        (DeviceOrientation::Portrait, StillRotation::Clockwise270),
        (DeviceOrientation::LandscapeRight, StillRotation::Clockwise180),
        (DeviceOrientation::PortraitUpsideDown, StillRotation::Clockwise90),
        (DeviceOrientation::LandscapeLeft, StillRotation::None),
    ];
    for (orientation, expected) in table_front {
        let mut still = plan(1, 1);
        still.orientation = orientation;
        let facts = build_facts(
            &still,
            &front_passport,
            choice(&front_passport),
            &params(&front_passport, 0.0),
            not_applied(),
        )
        .expect("факты");
        assert_eq!(facts.rotation, expected, "фронтальная, {orientation:?}");
    }
}

#[test]
fn refusals_are_named() {
    let mut rig = StreamRig::with_ring(MAIN);
    let passport = passport_of(&mut rig);
    let still = plan(1, 1);

    let without_details = RawFramePassport {
        capture: FrameCaptureMeta::Missing {
            reason: MissingCaptureMeta::PairingTimedOut,
        },
        ..passport.clone()
    };
    assert_eq!(
        build_facts(
            &still,
            &without_details,
            choice(&passport),
            &params(&passport, 0.0),
            not_applied()
        )
        .unwrap_err(),
        FactsRefusal::NoCaptureDetails
    );

    let mut unknown_camera = super::super::test_support::stream_passport("nowhere");
    unknown_camera.camera = camera_id("nowhere");
    let foreign = RawFramePassport {
        stream: Arc::new(unknown_camera),
        ..passport.clone()
    };
    assert_eq!(
        build_facts(
            &still,
            &foreign,
            choice(&passport),
            &params(&passport, 0.0),
            not_applied()
        )
        .unwrap_err(),
        FactsRefusal::CameraNotInCatalog
    );

    let mut tiny = super::super::test_support::stream_passport(MAIN);
    tiny.valid_area.size = PixelSize::new(2, 2);
    let tiny_passport = RawFramePassport {
        stream: Arc::new(tiny),
        ..passport.clone()
    };
    let mut zoomed = plan(1, 1);
    zoomed.framing = framing(CropAspect::SensorNative, 8.0);
    assert!(matches!(
        build_facts(
            &zoomed,
            &tiny_passport,
            choice(&passport),
            &params(&passport, 0.0),
            not_applied()
        )
        .unwrap_err(),
        FactsRefusal::Crop(_)
    ));
    let _ = stamp(0);
}

/// Карта с разными усилениями по каналам и точкам: перепутанный канал или порядок точек ловится сравнением.
fn asymmetric_map() -> Arc<LensShadingGainMap> {
    let points = (0..6)
        .map(|index| {
            let base = 1.0 + index as f32;
            [base, base + 0.25, base + 0.5, base + 0.75]
        })
        .collect();
    Arc::new(LensShadingGainMap::new(3, 2, points).expect("карта"))
}

#[test]
fn applied_step_hands_the_exact_map_to_the_facts() {
    let map = asymmetric_map();
    let applied = applied_lens_shading(&LensShadingStep::Apply(Arc::clone(&map))).expect("решение");
    let grid = applied.grid().expect("карта применена");
    assert_eq!((grid.columns(), grid.rows()), (3, 2));
    assert_eq!(grid.points(), map.points());

    // И попадает в факты кадра именно через `build_facts`, не теряясь по дороге.
    let mut rig = StreamRig::with_ring(MAIN);
    let passport = passport_of(&mut rig);
    let facts = build_facts(
        &plan(1, 1),
        &passport,
        choice(&passport),
        &params(&passport, 0.0),
        applied.clone(),
    )
    .expect("факты");
    assert_eq!(facts.lens_shading, applied);
}

#[test]
fn skipped_step_names_its_reason_and_carries_no_grid() {
    for (skip, reason) in [
        (
            LensShadingSkipReason::NotCalibrated,
            LensShadingNotAppliedReason::NotCalibrated,
        ),
        (
            LensShadingSkipReason::DisabledByControl,
            LensShadingNotAppliedReason::DisabledByControl,
        ),
    ] {
        let applied = applied_lens_shading(&LensShadingStep::Skip(skip)).expect("решение");
        assert_eq!(applied, AppliedLensShading::NotApplied(reason));
        assert!(applied.grid().is_none());
    }
}
