//! Тесты словаря вспышки: свет кадра по его результату, раздельные места замеров предвспышки,
//! ящик предвспышки рядом с ящиком предложений AE.

use std::time::Duration;

use super::*;
use crate::auto_exposure::{ExposureProposalChannel, SignalDelivery};
use crate::ids::CameraId;

fn result(mode: Option<FlashModeEcho>, state: Option<FlashState>) -> FlashResult {
    FlashResult { mode, state }
}

#[test]
fn frame_light_follows_the_frames_own_flash_result() {
    use FlashModeEcho::{Off, Single, Torch};
    use FlashState::{Charging, Fired, Partial, Ready, Unavailable};
    // Факты зонда 14d: кадр разового запроса — SINGLE + FIRED, кадры фонарика — TORCH + FIRED,
    // соседи — OFF + READY.
    let cases = [
        (result(Some(Single), Some(Fired)), FrameLight::FlashFired),
        (result(Some(Torch), Some(Fired)), FrameLight::TorchLit),
        (result(Some(Off), Some(Ready)), FrameLight::Ambient),
        // Эхо фонарика без состояния — светодиод включён запросом.
        (result(Some(Torch), None), FrameLight::TorchLit),
        // FIRED без эха режима — кадр вспышки (эхо не пришло).
        (result(None, Some(Fired)), FrameLight::FlashFired),
        // Вспышку просили, но горела ли — неизвестно: не подставлять как освещённый.
        (result(Some(Single), None), FrameLight::Unknown),
        (result(Some(Torch), Some(Partial)), FrameLight::PartiallyLit),
        (result(Some(Single), Some(Partial)), FrameLight::PartiallyLit),
        // Вспышка просили, но она заряжается или её нет — кадр без светодиода.
        (result(Some(Single), Some(Charging)), FrameLight::Ambient),
        (result(Some(Off), Some(Unavailable)), FrameLight::Ambient),
        (FlashResult::UNREPORTED, FrameLight::Ambient),
    ];
    for (flash, expected) in cases {
        assert_eq!(flash.light(), expected, "{flash:?}");
    }
}

#[test]
fn only_ambient_frames_feed_viewfinder_smoothing() {
    assert!(FrameLight::Ambient.feeds_viewfinder_smoothing());
    for lit in [
        FrameLight::TorchLit,
        FrameLight::FlashFired,
        FrameLight::PartiallyLit,
        FrameLight::Unknown,
    ] {
        assert!(!lit.feeds_viewfinder_smoothing(), "{lit:?}");
    }
}

#[test]
fn flash_share_is_clamped_to_zero_one_and_nan_is_zero() {
    assert_eq!(FlashShare::new(0.4).value(), 0.4);
    assert_eq!(FlashShare::new(-0.2).value(), 0.0);
    assert_eq!(FlashShare::new(1.7).value(), 1.0);
    assert_eq!(FlashShare::new(f64::NAN).value(), 0.0);
}

#[test]
fn flash_unit_defaults_to_absent_and_control_to_off() {
    assert_eq!(FlashUnit::default(), FlashUnit::Absent);
    assert!(!FlashUnit::Absent.is_present());
    assert!(FlashUnit::Present.is_present());
    assert_eq!(FlashControl::default(), FlashControl::Off);
    assert_eq!(FlashDirective::default(), FlashDirective::INACTIVE);
}

fn sample(light: PreflashLight, frame_nanos: i64, mid_gray_seconds_iso: f64) -> PreflashSample {
    PreflashSample {
        frame: SensorTimestamp::from_nanos(frame_nanos),
        measured_on: CameraTag::of(&CameraId::new("2").expect("имя камеры")),
        light,
        frame_product: ExposureProduct::from_result(Duration::from_millis(16), 800).expect("произведение кадра"),
        mid_gray_product: ExposureProduct::from_seconds_iso(mid_gray_seconds_iso).expect("произведение серого"),
        quality: MeasurementQuality::Measured,
    }
}

#[test]
fn preflash_samples_keep_ambient_and_torch_in_separate_places() {
    let mut samples = PreflashSamples::default();
    let ambient = sample(PreflashLight::Ambient, 1_000, 40.0);
    let torch = sample(PreflashLight::Torch, 2_000, 8.0);
    samples.record(ambient);
    samples.record(torch);
    // Новый кадр без фонарика заменяет только своё место, замер фонарика цел.
    let later_ambient = sample(PreflashLight::Ambient, 3_000, 41.0);
    samples.record(later_ambient);
    assert_eq!(samples.ambient, Some(later_ambient));
    assert_eq!(samples.torch, Some(torch));
}

#[test]
fn preflash_box_is_separate_from_proposals_and_nudges_the_engine() {
    let channel = ExposureProposalChannel::new();
    let mut samples = PreflashSamples::default();
    samples.record(sample(PreflashLight::Torch, 2_000, 8.0));
    let (generation, delivery) = channel.publish_preflash(samples);
    // Получателя толчка ещё нет — толчок учтён как пропущенный, а не потерян молча.
    assert_eq!(delivery, SignalDelivery::NotBoundYet);
    assert_eq!(channel.refresh_signal().missed_before_bind(), 1);
    assert_eq!(channel.preflash_newer_than(None), Some((generation, samples)));
    assert_eq!(channel.preflash_newer_than(Some(generation)), None);
    // Ящик предложений AE не тронут.
    assert!(channel.latest().is_none());
}
