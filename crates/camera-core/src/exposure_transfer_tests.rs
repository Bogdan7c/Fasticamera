//! Перенос произведения между камерами: числа OnePlus 11 и деградации при нехватке данных.

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::fixtures::{main_wide_camera, ultra_wide_camera};

use super::{CameraSide, ExposureTransfer, MissingSensitivityData, TransferTerm, exposure_transfer};

fn ev(factor: f64) -> f64 {
    factor.log2()
}

#[test]
fn main_to_ultra_wide_needs_about_one_point_two_two_ev_more() {
    let main = main_wide_camera().capabilities;
    let ultra = ultra_wide_camera().capabilities;
    let transfer = exposure_transfer(&main, &ultra);
    assert!(transfer.is_fully_measured());
    assert!(
        (ev(transfer.factor) - 1.22).abs() < 0.01,
        "получено {} EV",
        ev(transfer.factor)
    );
}

#[test]
fn aperture_and_pixel_terms_are_separate_and_the_reverse_direction_is_the_inverse() {
    let main = main_wide_camera().capabilities;
    let ultra = ultra_wide_camera().capabilities;
    // Только диафрагма: f/1,8 → f/2,2 даёт (2,2 / 1,8)².
    let mut same_pixels = ultra.clone();
    same_pixels.optics.sensor_physical_size = main.optics.sensor_physical_size;
    same_pixels.pixel_array = main.pixel_array;
    let aperture_only = exposure_transfer(&main, &same_pixels);
    assert!((aperture_only.factor - (2.2_f64 / 1.8).powi(2)).abs() < 1e-5);
    // Обратный перенос — обратный множитель.
    let back = exposure_transfer(&ultra, &main);
    let forward = exposure_transfer(&main, &ultra);
    assert!((back.factor * forward.factor - 1.0).abs() < 1e-9);
}

#[test]
fn missing_aperture_skips_only_that_term_with_a_reason() {
    let main = main_wide_camera().capabilities;
    let mut ultra = ultra_wide_camera().capabilities;
    ultra.optics.apertures_f_number.clear();
    let transfer = exposure_transfer(&main, &ultra);
    assert_eq!(
        transfer.aperture,
        TransferTerm::Skipped(MissingSensitivityData::Aperture(CameraSide::Target))
    );
    assert_eq!(transfer.pixel_pitch, TransferTerm::Applied);
    // Остался только член размера пикселя: (2,0 / 1,6)².
    assert!((transfer.factor - (2.0_f64 / 1.6).powi(2)).abs() < 1e-5);
    assert!(!transfer.is_fully_measured());
}

#[test]
fn missing_pixel_pitch_skips_only_that_term_and_no_data_means_factor_one() {
    let mut main = main_wide_camera().capabilities;
    let ultra = ultra_wide_camera().capabilities;
    main.pixel_array.width = 0;
    let transfer = exposure_transfer(&main, &ultra);
    assert_eq!(
        transfer.pixel_pitch,
        TransferTerm::Skipped(MissingSensitivityData::PixelPitch(CameraSide::Source))
    );
    assert!((transfer.factor - (2.2_f64 / 1.8).powi(2)).abs() < 1e-5);

    main.optics.apertures_f_number.clear();
    let nothing = exposure_transfer(&main, &ultra);
    assert_eq!(nothing.factor, 1.0);
}

#[test]
fn apply_scales_the_product_and_identity_keeps_it() {
    let main = main_wide_camera().capabilities;
    let ultra = ultra_wide_camera().capabilities;
    let product = ExposureProduct::from_seconds_iso(0.01).expect("произведение");
    let moved = exposure_transfer(&main, &ultra).apply(product);
    assert!((moved.ev_above(product) - 1.22).abs() < 0.01);
    assert_eq!(ExposureTransfer::IDENTITY.apply(product), product);
    assert_eq!(exposure_transfer(&main, &main).factor, 1.0);
}
