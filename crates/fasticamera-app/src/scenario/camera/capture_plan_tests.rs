//! Тесты плана шагов `capture`: порядок настроек и возврат вспышки в `Off`.

use super::*;

#[test]
fn flash_steps_come_before_the_burst_and_the_burst_runs_with_everything_off() {
    let steps = planned_steps();
    let names: Vec<&str> = steps.iter().map(|step| step.name).collect();
    assert_eq!(
        names,
        [
            STEP_SINGLE_HEIF,
            STEP_SINGLE_HEIF_DNG,
            STEP_SINGLE_FLASH_ON,
            STEP_SINGLE_FLASH_AUTO,
            STEP_BURST
        ]
    );
    let flash_on = steps[2];
    assert_eq!(flash_on.required_flash(), FlashMode::On);
    assert_eq!(flash_on.required_raw(), RawCapture::Off);
    assert_eq!(steps[3].required_flash(), FlashMode::Auto);
    let burst = steps[4];
    assert_eq!(
        (burst.required_flash(), burst.required_raw()),
        (FlashMode::Off, RawCapture::Off)
    );
}

#[test]
fn taps_to_flash_off_follow_the_screen_cycle() {
    assert_eq!(taps_to_flash_off(FlashMode::Off), 0);
    assert_eq!(taps_to_flash_off(FlashMode::On), 1);
    assert_eq!(taps_to_flash_off(FlashMode::Auto), 2);
}
