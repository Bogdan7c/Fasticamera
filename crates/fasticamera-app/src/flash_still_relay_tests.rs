//! Тесты моста итогов вспышки: срыв доходит до рендера с номером СВОЕГО снимка, удачный итог рендеру не уходит,
//! непривязанный мост и отказ рендера считаются, память итогов ограничена.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_contract::flash::{FlashFramePlan, FlashModeEcho, FlashResult, FlashShotId, FlashState, FlashStillFailure};
use camera_core::flash_status::FlashFrameResult;
use still_contract::plan::StillId;

use super::*;
use crate::still_test_support::{FakeRenderPort, LoggedCommand, RenderMode};

fn failed(shot: u64, failure: FlashStillFailure) -> FlashStillReport {
    FlashStillReport::Failed {
        shot: FlashShotId(shot),
        failure,
    }
}

fn fired(shot: u64) -> FlashStillReport {
    FlashStillReport::Fired {
        shot: FlashShotId(shot),
        frame: FlashFrameResult {
            frame_number: camera_contract::capture_result::FrameNumber(1),
            sensor_timestamp: None,
            flash: FlashResult {
                mode: Some(FlashModeEcho::Single),
                state: Some(FlashState::Fired),
            },
        },
        plan: FlashFramePlan {
            shot: FlashShotId(shot),
            ambient_frame: camera_contract::capture_result::SensorTimestamp::from_nanos(1_000),
            mid_gray_product: camera_contract::auto_exposure::ExposureProduct::from_seconds_iso(2.0).unwrap(),
            flash_share: camera_contract::flash::FlashShare::new(0.5),
            led_white: camera_contract::control::WhiteBalanceSetting {
                temperature: camera_contract::control::ColorTemperature::DAYLIGHT,
                tint: camera_contract::control::Tint::NEUTRAL,
            },
        },
        exposure_time: camera_contract::control::ExposureTime::new(std::time::Duration::from_millis(10)).unwrap(),
        iso: camera_contract::control::Iso::new(100).unwrap(),
        elapsed: std::time::Duration::from_millis(900),
    }
}

#[test]
fn a_failure_reaches_the_render_as_flash_failed_of_the_same_still() {
    let render = FakeRenderPort::new(RenderMode::Accepting);
    let relay = FlashStillRelay::default();
    relay.bind_render(render.clone());

    relay.flash_still_finished(&failed(7, FlashStillFailure::PreflashNotMeasured));

    assert_eq!(
        render.flash_failures(),
        vec![(StillId(7), FlashStillFailure::PreflashNotMeasured)]
    );
    assert_eq!(render.command_log(), vec![(LoggedCommand::FlashFailed, StillId(7))]);
    assert_eq!(relay.counters().failed_routed, 1);
}

#[test]
fn a_fired_report_is_counted_and_remembered_but_never_sent_to_the_render() {
    let render = FakeRenderPort::new(RenderMode::Accepting);
    let relay = FlashStillRelay::default();
    relay.bind_render(render.clone());

    relay.flash_still_finished(&fired(3));

    assert!(render.command_log().is_empty(), "рендер найдёт кадр в кольце сам");
    assert_eq!(relay.counters().fired, 1);
    assert_eq!(relay.reports(), vec![fired(3)]);
}

#[test]
fn a_failure_before_the_render_is_bound_is_counted_not_lost_silently() {
    let relay = FlashStillRelay::default();
    relay.flash_still_finished(&failed(1, FlashStillFailure::SessionLost));
    assert_eq!(relay.counters().unbound, 1);
    assert_eq!(relay.counters().failed_routed, 0);
    assert_eq!(relay.reports().len(), 1, "итог всё равно запомнен для диагностики");

    // После привязки следующий срыв уже идёт рендеру.
    let render = FakeRenderPort::new(RenderMode::Accepting);
    relay.bind_render(render.clone());
    relay.flash_still_finished(&failed(2, FlashStillFailure::SessionLost));
    assert_eq!(render.flash_failures().len(), 1);
    assert_eq!(relay.counters().unbound, 1, "счётчик не растёт после привязки");
}

#[test]
fn a_render_that_refuses_the_failure_is_counted() {
    for mode in [RenderMode::Gone, RenderMode::Unavailable] {
        let render = FakeRenderPort::new(mode);
        let relay = FlashStillRelay::default();
        relay.bind_render(render);
        relay.flash_still_finished(&failed(1, FlashStillFailure::SubmitFailed));
        assert_eq!(relay.counters().render_refused, 1, "{mode:?}");
        assert_eq!(relay.counters().failed_routed, 0, "{mode:?}");
    }
}

#[test]
fn remembered_reports_are_bounded_and_keep_the_newest() {
    let relay = FlashStillRelay::new(2);
    for shot in 1..=3 {
        relay.flash_still_finished(&fired(shot));
    }
    assert_eq!(relay.reports(), vec![fired(2), fired(3)]);
}
