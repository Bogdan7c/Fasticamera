use super::*;
use crate::fixtures;

fn plausibility() -> CalibrationPlausibility {
    CalibrationPlausibility {
        max_principal_point_offset: 0.15,
    }
}

fn active(width: u32, height: u32) -> PixelRect {
    PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(width, height),
    }
}

fn axis_of(descriptor: &crate::capabilities::CameraDescriptor) -> OpticalAxis {
    let capabilities = &descriptor.capabilities;
    capabilities
        .optics
        .calibration
        .optical_axis(capabilities.active_array, &plausibility())
}

#[test]
fn ultra_wide_axis_is_calibrated_and_shifted_left() {
    // Реальная калибровка ширика: c_x = 1759,3 из 4000 — ось левее центра на ≈ 6 % ширины.
    let OpticalAxis::Calibrated { point } = axis_of(&fixtures::ultra_wide_camera()) else {
        panic!("у ширика калибровка правдоподобная");
    };
    assert!((point.x() - 1759.3125 / 4000.0).abs() < 1e-6, "x {}", point.x());
    assert!((point.y() - 1494.7776 / 3000.0).abs() < 1e-6, "y {}", point.y());
}

#[test]
fn front_placeholder_at_origin_is_not_a_calibration() {
    // Заглушка HAL (0, 0) не должна стать «осью в углу сенсора».
    assert_eq!(
        axis_of(&fixtures::front_camera()),
        OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::PrincipalPointOutsideSensor
        }
    );
}

#[test]
fn missing_intrinsics_is_not_reported() {
    let calibration = LensCalibration::not_reported(active(4000, 3000));
    assert_eq!(
        calibration.optical_axis(active(4000, 3000), &plausibility()),
        OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::NotReported
        }
    );
}

fn calibration_with(focal: f32, principal_x: f32, principal_y: f32) -> LensCalibration {
    LensCalibration {
        intrinsics: Some(LensIntrinsics {
            focal_length_x_px: focal,
            focal_length_y_px: focal,
            principal_point_x_px: principal_x,
            principal_point_y_px: principal_y,
            skew: 0.0,
        }),
        pose: None,
        pre_correction_active_array: active(4000, 3000),
    }
}

#[test]
fn axis_far_from_center_is_rejected_with_offset() {
    // Смещение 800 px из 4000 = 0,2 > порога 0,15.
    let axis = calibration_with(1650.0, 2800.0, 1500.0).optical_axis(active(4000, 3000), &plausibility());
    let OpticalAxis::Uncalibrated {
        reason: UncalibratedReason::PrincipalPointTooFarFromCenter { offset },
    } = axis
    else {
        panic!("ожидался отказ по смещению, а не {axis:?}");
    };
    assert!((offset - 0.2).abs() < 1e-6, "смещение {offset}");
}

#[test]
fn non_positive_focal_length_is_rejected() {
    let axis = calibration_with(0.0, 2000.0, 1500.0).optical_axis(active(4000, 3000), &plausibility());
    assert_eq!(
        axis,
        OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::NonPositiveFocalLength
        }
    );
}

#[test]
fn pre_correction_offset_is_subtracted() {
    // Активная область внутри preCorrection со сдвигом (100, 50): ось 2100 → 2000 активной области.
    let mut calibration = calibration_with(1650.0, 2100.0, 1550.0);
    calibration.pre_correction_active_array = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(4200, 3100),
    };
    let active_inside = PixelRect {
        left: 100,
        top: 50,
        size: PixelSize::new(4000, 3000),
    };
    let OpticalAxis::Calibrated { point } = calibration.optical_axis(active_inside, &plausibility()) else {
        panic!("калибровка правдоподобная");
    };
    assert!(
        (point.x() - 0.5).abs() < 1e-6 && (point.y() - 0.5).abs() < 1e-6,
        "{point:?}"
    );
}

#[test]
fn degenerate_active_array_is_rejected() {
    let axis = calibration_with(1650.0, 2000.0, 1500.0).optical_axis(active(0, 3000), &plausibility());
    assert_eq!(
        axis,
        OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::DegenerateCoordinateSystem
        }
    );
}
