//! Ожидаемая цель нового потока (13b): подтяжка и «цель показа» до первого замера, без предложений движку.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::auto_exposure::ExposureDirective;

use super::tests::{applied, directive, ts, whole_frame_measurement};
use super::*;
use crate::exposure::test_support::{product, test_camera};

fn fresh_stream_with_request(applied_seconds_iso: f64) -> AutoExposureController {
    let mut controller = AutoExposureController::new(ExposureControllerConfig::default()).unwrap();
    controller.set_directive(ExposureDirective {
        applied: applied(applied_seconds_iso, 0.0),
        ..directive()
    });
    controller.begin_stream();
    controller
}

#[test]
fn the_expected_target_drives_the_display_gain_until_the_first_measurement() {
    let mut controller = fresh_stream_with_request(0.02);
    assert_eq!(
        controller.display_gain(Some(product(0.02))),
        isp_model::params::ExposureGain::UNITY
    );
    // Ожидаем 0,028 с·ISO (+0,49 EV к кадру), предел подтяжки конфига ±0,5 EV.
    controller.expect_target(Some(product(0.028)));
    assert!(controller.is_display_target_expected());
    let gain_ev = f64::from(controller.display_gain(Some(product(0.02))).ev());
    assert!(
        (gain_ev - (0.028_f64 / 0.02).log2()).abs() < 0.02,
        "подтяжка {gain_ev} EV"
    );
    assert_eq!(controller.target_log2(), None, "собственной цели всё ещё нет");
}

#[test]
fn the_first_measurement_replaces_the_expected_target() {
    let mut controller = fresh_stream_with_request(0.02);
    controller.expect_target(Some(product(0.5)));
    assert_eq!(
        controller.ingest(&whole_frame_measurement(0, 0.02)),
        IngestOutcome::Accepted
    );
    assert!(!controller.is_display_target_expected());
    let shown = controller.display_target_log2().unwrap();
    assert!(
        (shown - 0.02_f64.log2()).abs() < 1e-9,
        "показ идёт по замеру, а не по ожиданию"
    );
}

#[test]
fn the_expected_target_never_becomes_a_proposal() {
    let mut controller = fresh_stream_with_request(0.02);
    controller.expect_target(Some(product(0.5)));
    controller.advance(ts(0));
    assert!(controller.proposal(ts(0), test_camera()).is_none());
    assert_eq!(controller.counters().proposals_published, 0);
}

#[test]
fn a_new_stream_drops_the_expected_target_and_none_clears_it() {
    let mut controller = fresh_stream_with_request(0.02);
    controller.expect_target(Some(product(0.5)));
    controller.begin_stream();
    assert_eq!(controller.display_target_log2(), None);
    controller.expect_target(Some(product(0.5)));
    controller.expect_target(None);
    assert_eq!(controller.display_target_log2(), None);
}

#[test]
fn a_proposal_names_the_camera_it_was_measured_on() {
    let mut controller = fresh_stream_with_request(0.02);
    controller.ingest(&whole_frame_measurement(0, 0.02));
    controller.advance(ts(0));
    assert_eq!(
        controller.proposal(ts(0), test_camera()).unwrap().measured_on,
        test_camera()
    );
}
