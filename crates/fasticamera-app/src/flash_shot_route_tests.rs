//! Тесты выбора маршрута снимка: режим экрана × совет камеры.

use camera_core::flash_status::FlashAdviceUnavailable;

use super::*;

const SHOT: FlashShotId = FlashShotId(9);

#[test]
fn off_and_on_ignore_the_advice_and_never_read_it() {
    let off = FlashRoute::decide(FlashMode::Off, || panic!("Off не читает совет камеры"));
    assert_eq!(off, FlashRoute::ShownAtPress(NoFlashReason::ModeOff));
    let on = FlashRoute::decide(FlashMode::On, || panic!("On не читает совет камеры"));
    assert_eq!(on, FlashRoute::FlashFrame(FlashReason::ModeOn));
}

#[test]
fn auto_follows_the_advice_and_names_the_reason_when_it_is_absent() {
    let decide = |advice| FlashRoute::decide(FlashMode::Auto, || advice);
    assert_eq!(
        decide(FlashAdvice::Fire),
        FlashRoute::FlashFrame(FlashReason::AutoAdvisedFire)
    );
    assert_eq!(
        decide(FlashAdvice::NotNeeded),
        FlashRoute::ShownAtPress(NoFlashReason::AutoNotNeeded)
    );
    assert_eq!(
        decide(FlashAdvice::Unavailable {
            reason: FlashAdviceUnavailable::NoMeasurement
        }),
        FlashRoute::ShownAtPress(NoFlashReason::AutoAdviceUnavailable(
            FlashAdviceUnavailable::NoMeasurement
        ))
    );
}

#[test]
fn only_the_flash_route_asks_the_render_for_the_flash_frame_of_its_shot() {
    let flash = FlashRoute::FlashFrame(FlashReason::ModeOn);
    assert!(flash.uses_flash());
    assert_eq!(flash.selection(SHOT), FrameSelection::FlashFrame { shot: SHOT });
    let shown = FlashRoute::ShownAtPress(NoFlashReason::ModeOff);
    assert!(!shown.uses_flash());
    assert_eq!(shown.selection(SHOT), FrameSelection::ShownAtPress);
}
