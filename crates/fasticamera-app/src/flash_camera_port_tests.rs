//! Тесты шва к камере для вспышки на НАСТОЯЩЕМ движке с fake-бэкендом: статус читается из движка, команда доходит,
//! остановленный движок даёт отказ доставки, соглашение о номерах обратимо.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_contract::control::CaptureIntent;
use camera_contract::flash::FlashUnit;
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_core::flash_status::FlashCounters;

use super::*;
use crate::camera_test_support::FakeCameraRig;

fn stream_wide(rig: &FakeCameraRig) {
    // Android не даёт камеру фоновому приложению: переводим движок на передний план.
    rig.services.app_resumed();
    let handle = rig.handle();
    handle.open(CameraTarget::Role(LensRole::Wide)).unwrap();
    handle
        .configure(StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::FourByThree,
            frame_rate: TargetFrameRate::new(30).unwrap(),
        })
        .unwrap();
    handle.start_streaming(CaptureIntent::Preview).unwrap();
    rig.wait_streaming();
}

#[test]
fn still_and_flash_shot_numbers_are_the_same_number_in_both_directions() {
    assert_eq!(flash_shot_of(StillId(17)), FlashShotId(17));
    assert_eq!(still_of_flash_shot(FlashShotId(17)), StillId(17));
}

#[test]
fn status_reports_the_flash_of_the_active_camera_and_the_command_reaches_the_engine() {
    let rig = FakeCameraRig::new();
    let port = CameraHandleFlashPort::new(rig.handle());
    assert_eq!(port.flash_status().unit, FlashUnit::Absent, "камеры ещё нет");

    stream_wide(&rig);
    let status = rig
        .handle()
        .wait_until_status(std::time::Duration::from_secs(5), |status| {
            status.three_a.flash.unit == FlashUnit::Present
        });
    assert!(status.is_some(), "у основной камеры образцов есть вспышка");
    assert_eq!(port.flash_status().unit, FlashUnit::Present);

    assert_eq!(port.capture_flash_still(FlashShotId(5)), Ok(()));
    // Команда дошла до движка: он её посчитал (итог — отдельное событие, здесь важна доставка).
    let counted = rig
        .handle()
        .wait_until_status(std::time::Duration::from_secs(5), |status| {
            status.three_a.flash.counters != FlashCounters::default()
        });
    assert!(counted.is_some(), "движок принял команду снимка со вспышкой");
    assert!(counted.unwrap().three_a.flash.counters.requested >= 1);
}

#[test]
fn a_flash_the_real_engine_cannot_shoot_comes_back_to_the_render_as_flash_failed_of_the_same_still() {
    use crate::still_test_support::{FakeRenderPort, RenderMode};
    // Настоящий движок на fake-бэкенде, настоящий мост итогов, рендер-подделка: ни одного замера предвспышки тест
    // не публикует, поэтому движок обязан сам закончить снимок срывом (а не молчать), и срыв должен дойти до рендера.
    let rig = FakeCameraRig::new();
    let render = FakeRenderPort::new(RenderMode::Accepting);
    rig.flash_relay.bind_render(render.clone());
    stream_wide(&rig);
    let port = CameraHandleFlashPort::new(rig.handle());

    port.capture_flash_still(flash_shot_of(StillId(5))).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while render.flash_failures().is_empty() {
        assert!(std::time::Instant::now() < deadline, "срыв не дошёл до рендера");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let failures = render.flash_failures();
    assert_eq!(failures.len(), 1, "ровно один итог на снимок");
    assert_eq!(failures[0].0, StillId(5), "номер снимка службы = номер снимка вспышки");
    assert_ne!(
        failures[0].1,
        camera_contract::flash::FlashStillFailure::CommandNotDelivered,
        "причина — от движка, а не «команда не доставлена»"
    );
    assert_eq!(rig.flash_relay.counters().failed_routed, 1);
}
