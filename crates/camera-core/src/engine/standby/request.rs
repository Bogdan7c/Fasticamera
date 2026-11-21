//! Повторяющийся запрос тёплой камеры.
//!
//! Тёплая ничего не показывает, но после повышения её первый кадр должен быть похож на кадр
//! основной по яркости и быть в фокусе. Поэтому:
//! * экспозиция — произведение «выдержка × ISO» основной, перенесённое на тёплую через
//!   [`crate::exposure_transfer`] и разложенное на выдержку и ISO в пределах тёплой;
//! * фокус — непрерывный AF (у камеры с фиксированным фокусом — без AF);
//! * запрос пересчитывается, только когда произведение основной сдвинулось не меньше порога
//!   `three_a.exposure.replan_min_change_ev` (тот же порог, что у нашего AE), а не на каждый кадр.

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::capabilities::{CameraCapabilities, FocusCapability, FrameDuration};
use camera_contract::control::{CaptureIntent, ExposureTime, Iso};
use camera_contract::three_a::AeAfLock;

use crate::exposure_transfer::exposure_transfer;
use crate::request_controls::{ExposurePlan, FocusPlan, RequestControls};
use crate::three_a_exposure_limits::{longest_usable_shutter, manual_sensor_ranges};
use crate::three_a_own::{Anchor, SplitLimits, split};

/// Запрос тёплой, как его хочет движок сейчас.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct IssuedRequest {
    pub(super) intent: CaptureIntent,
    pub(super) controls: RequestControls,
    /// Произведение основной, из которого посчитана экспозиция; `None` — основная не на `Sensor`
    /// (вендорский авто) или у тёплой нет ручного сенсора.
    pub(super) source_product: Option<ExposureProduct>,
}

/// Управляющая часть запроса тёплой по запросу основной. Чистая функция.
///
/// `frame_duration` — длительность кадра потока тёплой: выдержка не длиннее кадра.
pub(super) fn build_standby_request(
    primary_controls: &RequestControls,
    primary: &CameraCapabilities,
    standby: &CameraCapabilities,
    frame_duration: FrameDuration,
) -> (RequestControls, Option<ExposureProduct>) {
    let (exposure, source_product) = match primary_controls.exposure {
        ExposurePlan::Sensor { exposure_time, iso } => {
            let source = ExposureProduct::from_sensor(exposure_time, iso);
            let transferred = exposure_transfer(primary, standby).apply(source);
            match sensor_plan_for(transferred, standby, frame_duration) {
                Some(plan) => (plan, Some(source)),
                // У тёплой нет ручного сенсора: пусть сходится вендорский авто.
                None => (vendor_auto(0), None),
            }
        }
        // Вендорский авто основной: у тёплой тоже авто с той же компенсацией, но без блокировки
        // и региона тапа — они относятся к основной.
        ExposurePlan::VendorAuto { compensation_steps, .. } => (vendor_auto(compensation_steps), None),
    };
    let focus = match standby.focus {
        FocusCapability::Fixed => FocusPlan::FixedLens,
        FocusCapability::Adjustable { .. } => FocusPlan::Continuous { region: None },
    };
    (RequestControls { exposure, focus }, source_product)
}

fn vendor_auto(compensation_steps: i32) -> ExposurePlan {
    ExposurePlan::VendorAuto {
        compensation_steps,
        lock: AeAfLock::Unlocked,
        region: None,
    }
}

/// `Sensor`-план с нужным произведением в пределах камеры; `None` — ручного сенсора нет
/// или пределы нелепые.
fn sensor_plan_for(
    product: ExposureProduct,
    capabilities: &CameraCapabilities,
    frame_duration: FrameDuration,
) -> Option<ExposurePlan> {
    let (iso_range, shutter_range) = manual_sensor_ranges(capabilities)?;
    let limits = SplitLimits {
        iso_min: iso_range.min,
        iso_max: iso_range.max,
        shutter_shortest: shutter_range.shortest,
        shutter_cap: longest_usable_shutter(shutter_range, frame_duration),
    };
    // Мерцание тёплой не защищаем: кадры не показываются, а после повышения запрос перестроит 3A.
    let divided = split(product.seconds_iso(), Anchor::Free, limits, None);
    Some(ExposurePlan::Sensor {
        exposure_time: ExposureTime::new(divided.shutter).ok()?,
        iso: Iso::new(divided.iso).ok()?,
    })
}

/// Нужно ли перепоставить запрос тёплой, если сейчас стоит `issued`, а хочется `wanted`.
///
/// Перестраиваем при смене вида плана или фокуса, а в пределах `Sensor` — только когда
/// произведение основной сдвинулось не меньше `threshold_ev` (мелкий дрейф AE не дёргает бэкенд).
pub(super) fn needs_reissue(issued: &IssuedRequest, wanted: &IssuedRequest, threshold_ev: f64) -> bool {
    if issued.intent != wanted.intent || issued.controls.focus != wanted.controls.focus {
        return true;
    }
    match (issued.source_product, wanted.source_product) {
        (Some(was), Some(now)) => now.ev_above(was).abs() >= threshold_ev,
        (None, None) => issued.controls != wanted.controls,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use camera_contract::capabilities::FrameDuration;
    use camera_contract::fixtures::{front_camera, main_wide_camera, ultra_wide_camera};

    use super::*;

    fn frame_60() -> FrameDuration {
        FrameDuration::from_nanos(16_666_667)
    }

    fn sensor(millis: u64, iso: u32) -> ExposurePlan {
        ExposurePlan::Sensor {
            exposure_time: ExposureTime::new(Duration::from_millis(millis)).expect("выдержка"),
            iso: Iso::new(iso).expect("ISO"),
        }
    }

    fn controls(exposure: ExposurePlan) -> RequestControls {
        RequestControls {
            exposure,
            focus: FocusPlan::Continuous { region: None },
        }
    }

    #[test]
    fn sensor_exposure_is_carried_to_the_standby_camera_with_the_sensitivity_ratio() {
        let main = main_wide_camera().capabilities;
        let ultra = ultra_wide_camera().capabilities;
        let (request, source) = build_standby_request(&controls(sensor(8, 100)), &main, &ultra, frame_60());
        let ExposurePlan::Sensor { exposure_time, iso } = request.exposure else {
            panic!("ожидался Sensor, получено {:?}", request.exposure);
        };
        let carried = ExposureProduct::from_sensor(exposure_time, iso);
        let primary = ExposureProduct::from_sensor(
            ExposureTime::new(Duration::from_millis(8)).expect("выдержка"),
            Iso::new(100).expect("ISO"),
        );
        assert_eq!(source, Some(primary));
        assert!(
            (carried.ev_above(primary) - 1.22).abs() < 0.02,
            "{} EV",
            carried.ev_above(primary)
        );
        assert_eq!(request.focus, FocusPlan::Continuous { region: None });
    }

    #[test]
    fn vendor_auto_primary_gives_unlocked_auto_without_the_tap_region() {
        let main = main_wide_camera().capabilities;
        let ultra = ultra_wide_camera().capabilities;
        let primary = controls(ExposurePlan::VendorAuto {
            compensation_steps: 2,
            lock: AeAfLock::Locked,
            region: None,
        });
        let (request, source) = build_standby_request(&primary, &main, &ultra, frame_60());
        assert_eq!(request.exposure, vendor_auto(2));
        assert_eq!(source, None);
    }

    #[test]
    fn fixed_focus_camera_gets_fixed_lens_not_continuous_af() {
        let main = main_wide_camera().capabilities;
        let front = front_camera().capabilities;
        let (request, _) = build_standby_request(&controls(sensor(8, 100)), &main, &front, frame_60());
        assert_eq!(request.focus, FocusPlan::FixedLens);
    }

    #[test]
    fn camera_without_manual_sensor_falls_back_to_vendor_auto() {
        let main = main_wide_camera().capabilities;
        let mut ultra = ultra_wide_camera().capabilities;
        ultra.manual_sensor = camera_contract::capabilities::ManualSensorRanges::NotSupported;
        let (request, source) = build_standby_request(&controls(sensor(8, 100)), &main, &ultra, frame_60());
        assert_eq!(request.exposure, vendor_auto(0));
        assert_eq!(source, None);
    }

    fn issued(product: Option<f64>) -> IssuedRequest {
        IssuedRequest {
            intent: CaptureIntent::Preview,
            controls: controls(sensor(8, 100)),
            source_product: product.map(|value| ExposureProduct::from_seconds_iso(value).expect("произведение")),
        }
    }

    #[test]
    fn small_exposure_drift_does_not_reissue_but_a_visible_step_does() {
        let threshold: f64 = 1.0 / 48.0;
        let standing = issued(Some(0.8));
        // ~0,5 порога: не перестраиваем.
        assert!(!needs_reissue(
            &standing,
            &issued(Some(0.8 * (threshold / 2.0).exp2())),
            threshold
        ));
        // ~2 порога: перестраиваем.
        assert!(needs_reissue(
            &standing,
            &issued(Some(0.8 * (threshold * 2.0).exp2())),
            threshold
        ));
        // Вид плана сменился (Sensor → авто): перестраиваем.
        assert!(needs_reissue(&standing, &issued(None), threshold));
    }
}
