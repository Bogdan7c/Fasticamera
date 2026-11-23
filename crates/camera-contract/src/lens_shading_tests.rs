use std::sync::Arc;

use super::{
    GAIN_CHANNELS, InvalidGainGrid, InvalidSensorArea, LensShadingCalibration, LensShadingCalibrationState,
    LensShadingCalibrations, LensShadingGainGrid, NotCalibratedReason, ValidAreaOnSensor,
};
use crate::geometry::{PixelRect, PixelSize};
use crate::ids::CameraId;

fn camera(id: &str) -> CameraId {
    CameraId::new(id).unwrap()
}

fn unit_grid(columns: u32, rows: u32) -> LensShadingGainGrid {
    LensShadingGainGrid::new(columns, rows, vec![[1.0; GAIN_CHANNELS]; (columns * rows) as usize]).unwrap()
}

fn calibration(id: &str, active_array: PixelSize) -> LensShadingCalibration {
    LensShadingCalibration {
        camera: camera(id),
        active_array,
        grid: unit_grid(33, 25),
    }
}

#[test]
fn grid_rejects_every_kind_of_malformed_input() {
    assert_eq!(
        LensShadingGainGrid::new(1, 5, vec![[1.0; 4]; 5]),
        Err(InvalidGainGrid::GridTooSmall { columns: 1, rows: 5 })
    );
    assert_eq!(
        LensShadingGainGrid::new(2, 2, vec![[1.0; 4]; 3]),
        Err(InvalidGainGrid::PointCountMismatch {
            columns: 2,
            rows: 2,
            expected: 4,
            actual: 3
        })
    );
    // Усиление меньше 1 — калибровка только поднимает тёмное, а не затемняет.
    assert_eq!(
        LensShadingGainGrid::new(2, 2, vec![[1.0; 4], [1.0; 4], [1.0, 0.99, 1.0, 1.0], [1.0; 4]]),
        Err(InvalidGainGrid::InvalidGain { point: 2, gain: 0.99 })
    );
    let nan = LensShadingGainGrid::new(2, 2, vec![[1.0; 4], [1.0, 1.0, f32::NAN, 1.0], [1.0; 4], [1.0; 4]]);
    assert!(
        matches!(nan, Err(InvalidGainGrid::InvalidGain { point: 1, .. })),
        "{nan:?}"
    );
    let infinite = LensShadingGainGrid::new(2, 2, vec![[f32::INFINITY, 1.0, 1.0, 1.0]; 4]);
    assert!(matches!(infinite, Err(InvalidGainGrid::InvalidGain { point: 0, .. })));
}

#[test]
fn grid_exposes_points_row_major_and_rejects_outside_points() {
    let points = vec![
        [1.0, 1.1, 1.2, 1.3],
        [2.0, 2.1, 2.2, 2.3],
        [3.0, 3.1, 3.2, 3.3],
        [4.0, 4.1, 4.2, 4.3],
        [5.0, 5.1, 5.2, 5.3],
        [6.0, 6.1, 6.2, 6.3],
    ];
    let grid = LensShadingGainGrid::new(3, 2, points.clone()).unwrap();
    assert_eq!((grid.columns(), grid.rows()), (3, 2));
    assert_eq!(grid.points(), points.as_slice());
    assert_eq!(
        grid.point(2, 0),
        Some([3.0, 3.1, 3.2, 3.3]),
        "последний столбец первой строки"
    );
    assert_eq!(
        grid.point(0, 1),
        Some([4.0, 4.1, 4.2, 4.3]),
        "первый столбец второй строки"
    );
    assert_eq!(grid.point(3, 0), None);
    assert_eq!(grid.point(0, 2), None);
}

#[test]
fn debug_output_is_compact_and_does_not_dump_points() {
    let state = LensShadingCalibrationState::Calibrated(Arc::new(calibration("2", PixelSize::new(4096, 3072))));
    let text = format!("{state:?} {:?}", unit_grid(33, 25));
    assert!(text.contains("33×25"), "{text}");
    assert!(text.len() < 200, "Debug раздулся до {} байт: {text}", text.len());
}

#[test]
fn state_serializes_as_summary_without_points() {
    let calibrated = LensShadingCalibrationState::Calibrated(Arc::new(calibration("2", PixelSize::new(4096, 3072))));
    let json = serde_json::to_value(&calibrated).unwrap();
    assert_eq!(json["state"], "calibrated");
    assert_eq!(json["camera"], "2");
    assert_eq!(json["grid_columns"], 33);
    assert!(json.get("points").is_none());
    let missing = LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::NoCalibrationForCamera);
    let json = serde_json::to_value(&missing).unwrap();
    assert_eq!(json["state"], "not_calibrated");
    assert_eq!(json["reason"], "no_calibration_for_camera");
}

#[test]
fn calibrated_state_equality_is_by_value_so_cache_keys_stay_meaningful() {
    let size = PixelSize::new(4096, 3072);
    let first = Arc::new(calibration("2", size));
    let same_pointer = LensShadingCalibrationState::Calibrated(Arc::clone(&first));
    let equal_value = LensShadingCalibrationState::Calibrated(Arc::new(calibration("2", size)));
    let other_camera = LensShadingCalibrationState::Calibrated(Arc::new(calibration("3", size)));
    assert_eq!(same_pointer, LensShadingCalibrationState::Calibrated(first));
    assert_eq!(same_pointer, equal_value);
    assert_ne!(same_pointer, other_camera);
    assert_ne!(
        same_pointer,
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::NoCalibrationForCamera)
    );
}

#[test]
fn set_attaches_matching_calibration_and_reports_reasons_otherwise() {
    let full = PixelSize::new(4096, 3072);
    let set = LensShadingCalibrations::new(vec![
        calibration("2", full),
        calibration("3", PixelSize::new(4000, 3000)),
    ]);
    let main = set.state_for(&camera("2"), full);
    assert_eq!(main.calibration().map(|found| found.camera.clone()), Some(camera("2")));
    assert_eq!(
        set.state_for(&camera("9"), full),
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::NoCalibrationForCamera)
    );
    // Камера «3» снята для 4000×3000, а у камеры другая активная область — сетке верить нельзя.
    assert_eq!(
        set.state_for(&camera("3"), full),
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::ActiveArrayMismatch {
            calibration: PixelSize::new(4000, 3000),
            camera: full,
        })
    );
    assert_eq!(
        LensShadingCalibrations::device_not_in_profile().state_for(&camera("2"), full),
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::DeviceNotInProfile)
    );
    assert!(LensShadingCalibrations::none().is_empty());
    assert_eq!(set.cameras().count(), 2);
}

#[test]
fn sensor_area_validates_the_rectangle() {
    let array = PixelSize::new(4096, 3072);
    let band = PixelRect {
        left: 0,
        top: 384,
        size: PixelSize::new(4096, 2304),
    };
    let area = ValidAreaOnSensor::new(array, band).unwrap();
    assert_eq!(area.rect, band);
    assert_eq!(
        ValidAreaOnSensor::whole_active_array(array).rect,
        PixelRect {
            left: 0,
            top: 0,
            size: array
        }
    );
    let too_low = PixelRect { top: 800, ..band };
    assert_eq!(
        ValidAreaOnSensor::new(array, too_low),
        Err(InvalidSensorArea {
            active_array: array,
            rect: too_low
        })
    );
    let empty = PixelRect {
        size: PixelSize::new(0, 10),
        ..band
    };
    assert!(ValidAreaOnSensor::new(array, empty).is_err());
}

#[test]
fn blending_toward_unity_is_total_and_keeps_every_gain_valid() {
    let grid = LensShadingGainGrid::new(2, 2, vec![[1.0, 2.0, 3.0, 5.0]; 4]).unwrap();
    assert_eq!(grid.blended_toward_unity(1.0), grid, "вес 1 — без изменений");
    assert_eq!(grid.blended_toward_unity(0.0).points(), &[[1.0; 4]; 4]);
    assert_eq!(grid.blended_toward_unity(0.5).points()[0], [1.0, 1.5, 2.0, 3.0]);
    // Вес вне диапазона зажимается, NaN — это 0: инвариант «усиление ≥ 1» не ломается.
    assert_eq!(grid.blended_toward_unity(7.0), grid);
    assert_eq!(grid.blended_toward_unity(-1.0).points(), &[[1.0; 4]; 4]);
    assert_eq!(grid.blended_toward_unity(f32::NAN).points(), &[[1.0; 4]; 4]);
}
