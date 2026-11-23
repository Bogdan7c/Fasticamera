//! Виньетирование в снимке: факты кадра несут ту поправку, что применена к проявленной картинке
//! (калибровка потока → `Applied` с картой потока; нет калибровки или выключено → `NotApplied` с причиной).

use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::fixtures::camera_id;
use camera_contract::lens_shading::{LensShadingCalibration, LensShadingCalibrationState, LensShadingGainGrid};
use isp_model::lens_shading::LensShadingControl;
use still_contract::develop::DevelopEvent;
use still_contract::lens_shading::{AppliedLensShading, LensShadingNotAppliedReason};

use super::test_support::{EngineRig, FRAME_SIZE, MAIN, StreamRig, engine_rig, ms, plan, stream_passport};
use crate::backend::PastPresentation;
use crate::config::StillDevelopConfig;

/// Сетка 2×2, где каждое число уникально: перепутанный канал или угол виден сразу.
fn asymmetric_grid() -> LensShadingGainGrid {
    LensShadingGainGrid::new(
        2,
        2,
        vec![
            [4.0, 3.5, 3.25, 3.0],
            [2.5, 2.25, 2.0, 1.75],
            [2.75, 2.375, 2.125, 1.875],
            [1.0, 1.125, 1.25, 1.5],
        ],
    )
    .unwrap()
}

fn calibrated_passport() -> camera_contract::frame_passport::StreamPassport {
    let mut passport = stream_passport(MAIN);
    passport.lens_shading = LensShadingCalibrationState::Calibrated(Arc::new(LensShadingCalibration {
        camera: camera_id(MAIN),
        active_array: FRAME_SIZE,
        grid: asymmetric_grid(),
    }));
    passport
}

/// Снимок на одном показанном кадре; возвращает факты `Secured`.
fn secured_lens_shading(stream: StreamRig, control: LensShadingControl) -> AppliedLensShading {
    let mut stream = stream;
    let frames: Vec<SensorTimestamp> = stream.offer_many(3);
    let mut rig: EngineRig = engine_rig(StillDevelopConfig::default(), stream);
    rig.engine = rig.engine.with_lens_shading(control);
    let feed = rig.stream.feed_for(&frames, 0);
    rig.engine.ingest_feed(feed);
    let timings: Vec<PastPresentation> = (0..3)
        .map(|index| PastPresentation {
            present_id: index,
            frame_index: u64::from(index),
            actual_present: ms(10 * i64::from(index)),
        })
        .collect();
    rig.engine.ingest_presentations(&timings);
    let sink = rig.begin(plan(1, 15));
    rig.run(3);
    match sink.take().first() {
        Some(DevelopEvent::Secured { facts, .. }) => facts.lens_shading.clone(),
        other => panic!("первым событием должен быть Secured, а пришло {other:?}"),
    }
}

#[test]
fn calibrated_stream_hands_its_exact_map_to_the_still_facts() {
    let applied = secured_lens_shading(
        StreamRig::with_ring_and_passport(calibrated_passport()),
        LensShadingControl::default(),
    );
    let grid = applied.grid().expect("снимок проявлен с коррекцией");
    // Полная сила и полезная область = активная область: карта потока равна калибровке точка в точку.
    assert_eq!(grid.as_ref(), &asymmetric_grid());
}

#[test]
fn stream_without_calibration_says_so_in_the_facts() {
    let applied = secured_lens_shading(StreamRig::with_ring(MAIN), LensShadingControl::default());
    assert_eq!(
        applied,
        AppliedLensShading::NotApplied(LensShadingNotAppliedReason::NotCalibrated)
    );
}

#[test]
fn disabled_control_wins_over_calibration_and_is_named() {
    let applied = secured_lens_shading(
        StreamRig::with_ring_and_passport(calibrated_passport()),
        LensShadingControl::Disabled,
    );
    assert_eq!(
        applied,
        AppliedLensShading::NotApplied(LensShadingNotAppliedReason::DisabledByControl)
    );
}
