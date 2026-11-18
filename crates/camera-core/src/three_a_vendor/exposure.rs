//! Временный вендорский AE (до 08a): компенсация, блокировка и регион через HAL, либо
//! полностью ручной сенсор. Полуавтомат честно отвергается — он требует нашего AE.

use camera_contract::capabilities::{FrameDuration, ManualSensorRanges};
use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::control::{Control, ExposureBias, ExposureTime, Iso};
use camera_contract::three_a::{AeAfLock, ExposureCompensationSupport, LockSupport, PointOfInterest};
use std::time::Duration;

use crate::command::UnsupportedThreeA;
use crate::request_controls::ExposurePlan;
use crate::three_a_control::{ExposureController, ExposureDecision, PlanRefresh, ThreeAContext};
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter};

/// Допуск, с которым дробное число шагов компенсации считается целым. `ExposureBias` хранится
/// в `f32` (точность ~1e-7 относительная), шаг HAL — 1/6 EV, поэтому 1e-4 шага — на порядки
/// больше шума округления (+0,5 EV при шаге 1/6 даёт ровно 3 шага) и на порядки меньше
/// настоящей разницы (+0,55 EV — 3,3 шага).
const WHOLE_STEP_TOLERANCE: f64 = 1e-4;

/// Временный исполнитель экспозиции на вендорском AE.
#[derive(Debug, Default, Clone, Copy)]
pub struct VendorExposure;

impl ExposureController for VendorExposure {
    fn plan(&mut self, context: &ThreeAContext<'_>) -> Result<ExposureDecision, UnsupportedThreeA> {
        let controls = &context.intent.controls;
        match (controls.iso, controls.shutter) {
            (Control::Auto, Control::Auto) => plan_vendor_auto(context),
            (Control::Manual(iso), Control::Manual(shutter)) => plan_manual_sensor(context, iso, shutter),
            // Вендорский AE не умеет «одно вручную, другое само»; подмена любой из сторон
            // скрыла бы от пользователя, что его значение не применено (решение 63).
            (Control::Auto, Control::Manual(_)) | (Control::Manual(_), Control::Auto) => {
                Err(UnsupportedThreeA::SemiAutomaticExposure)
            }
        }
    }

    fn observe_result(&mut self, _result: &CaptureResultMeta) {
        // Вендорскому AE результаты не нужны: HAL сам ведёт экспозицию.
    }

    fn refresh(&mut self) -> PlanRefresh {
        // Решений «со стороны» у вендорского AE нет, план меняют только команды.
        PlanRefresh::Unchanged
    }

    fn reset_for_new_stream(&mut self) {}
}

/// Обе величины на автомате: компенсация в шагах HAL, блокировка и регион замера.
fn plan_vendor_auto(context: &ThreeAContext<'_>) -> Result<ExposureDecision, UnsupportedThreeA> {
    let three_a = &context.capabilities.three_a;
    let lock = context.intent.lock;
    if lock == AeAfLock::Locked && three_a.ae_lock == LockSupport::NotAvailable {
        return Err(UnsupportedThreeA::ExposureLock);
    }
    let mut adjustments = Vec::new();
    let compensation_steps = compensation_in_steps(
        three_a.exposure_compensation,
        context.intent.controls.exposure_bias,
        &mut adjustments,
    );
    // Регион замера ставим, только если HAL объявил хотя бы один регион AE.
    let region = match context.intent.point {
        PointOfInterest::Point { region, .. } if three_a.max_regions.auto_exposure > 0 => Some(region),
        PointOfInterest::Point { .. } | PointOfInterest::WholeFrame => None,
    };
    Ok(ExposureDecision {
        plan: ExposurePlan::VendorAuto {
            compensation_steps,
            lock,
            region,
        },
        adjustments,
    })
}

/// EV → целые шаги HAL; всё, что не сошлось точно, попадает в `adjustments`.
fn compensation_in_steps(
    support: ExposureCompensationSupport,
    bias: ExposureBias,
    adjustments: &mut Vec<ControlAdjustment>,
) -> i32 {
    let requested_ev = f64::from(bias.ev());
    let (min_steps, max_steps, step) = match support {
        ExposureCompensationSupport::Supported {
            min_steps,
            max_steps,
            step_ev,
        } if step_ev.numerator > 0 && step_ev.denominator > 0 => (min_steps, max_steps, step_ev),
        // Камера компенсации не умеет (или шаг в характеристиках нелепый): применить нечего.
        ExposureCompensationSupport::Supported { .. } | ExposureCompensationSupport::NotSupported => {
            if requested_ev != 0.0 {
                adjustments.push(ControlAdjustment {
                    parameter: ControlParameter::ExposureBias,
                    requested: requested_ev,
                    applied: 0.0,
                    reason: AdjustmentReason::NotSupportedByCamera,
                });
            }
            return 0;
        }
    };
    // Шаг = numerator/denominator EV, значит шагов = ev × denominator / numerator
    // (умножение, а не деление на дробь: +0,5 EV при 1/6 даёт ровно 3,0).
    let exact_steps = requested_ev * f64::from(step.denominator) / f64::from(step.numerator);
    let rounded_steps = exact_steps.round();
    let clamped_steps = rounded_steps.clamp(f64::from(min_steps), f64::from(max_steps));
    let reason = if rounded_steps < f64::from(min_steps) {
        Some(AdjustmentReason::BelowCameraRange)
    } else if rounded_steps > f64::from(max_steps) {
        Some(AdjustmentReason::AboveCameraRange)
    } else if (exact_steps - rounded_steps).abs() > WHOLE_STEP_TOLERANCE {
        Some(AdjustmentReason::QuantizedToStep)
    } else {
        None
    };
    if let Some(reason) = reason {
        adjustments.push(ControlAdjustment {
            parameter: ControlParameter::ExposureBias,
            requested: requested_ev,
            applied: clamped_steps * f64::from(step.numerator) / f64::from(step.denominator),
            reason,
        });
    }
    // Диапазон шагов — `i32`, а `clamp` держит значение внутри него: приведение безопасно.
    clamped_steps as i32
}

/// Обе величины вручную: ISO и выдержка внутри диапазона камеры, выдержка не длиннее кадра.
fn plan_manual_sensor(
    context: &ThreeAContext<'_>,
    iso: Iso,
    shutter: ExposureTime,
) -> Result<ExposureDecision, UnsupportedThreeA> {
    let ManualSensorRanges::Supported {
        iso: iso_range,
        exposure_time: shutter_range,
        ..
    } = context.capabilities.manual_sensor
    else {
        return Err(UnsupportedThreeA::ManualSensor);
    };
    let mut adjustments = Vec::new();

    let applied_iso = clamp_with_note(
        iso.value(),
        iso_range.min,
        iso_range.max,
        ControlParameter::Iso,
        f64::from,
        &mut adjustments,
    );
    let mut applied_shutter = clamp_with_note(
        shutter.duration(),
        shutter_range.shortest,
        shutter_range.longest,
        ControlParameter::Shutter,
        |value| value.as_nanos() as f64,
        &mut adjustments,
    );
    // Выдержка не может быть длиннее кадра потока: 60 к/с держим только короткой выдержкой.
    let frame_limit = frame_limit(context.stream_frame_duration, shutter_range.shortest);
    if let Some(limit) = frame_limit.filter(|limit| applied_shutter > *limit) {
        adjustments.push(ControlAdjustment {
            parameter: ControlParameter::Shutter,
            requested: shutter.duration().as_nanos() as f64,
            applied: limit.as_nanos() as f64,
            reason: AdjustmentReason::LimitedByFrameDuration,
        });
        applied_shutter = limit;
    }
    // Компенсация при полностью ручной экспозиции не действует — говорим об этом явно.
    let bias_ev = f64::from(context.intent.controls.exposure_bias.ev());
    if bias_ev != 0.0 {
        adjustments.push(ControlAdjustment {
            parameter: ControlParameter::ExposureBias,
            requested: bias_ev,
            applied: 0.0,
            reason: AdjustmentReason::IgnoredWhileExposureFullyManual,
        });
    }
    // Нулевые границы диапазона — нелепые характеристики камеры, а не ошибка пользователя.
    let (Ok(applied_iso), Ok(applied_shutter)) = (Iso::new(applied_iso), ExposureTime::new(applied_shutter)) else {
        return Err(UnsupportedThreeA::ManualSensor);
    };
    Ok(ExposureDecision {
        plan: ExposurePlan::Sensor {
            exposure_time: applied_shutter,
            iso: applied_iso,
        },
        adjustments,
    })
}

/// Предел выдержки по длительности кадра; `None`, если кадр нулевой (HAL не ограничил) или
/// короче минимальной выдержки камеры (предел бессмыслен).
fn frame_limit(frame: FrameDuration, shortest: Duration) -> Option<Duration> {
    let limit = frame.as_duration();
    (limit >= shortest && !limit.is_zero()).then_some(limit)
}

/// Подтянуть значение к диапазону; если подтянули — записать, что и почему.
fn clamp_with_note<T: PartialOrd + Copy>(
    value: T,
    min: T,
    max: T,
    parameter: ControlParameter,
    to_number: impl Fn(T) -> f64,
    adjustments: &mut Vec<ControlAdjustment>,
) -> T {
    let (applied, reason) = if value < min {
        (min, AdjustmentReason::BelowCameraRange)
    } else if value > max {
        (max, AdjustmentReason::AboveCameraRange)
    } else {
        return value;
    };
    adjustments.push(ControlAdjustment {
        parameter,
        requested: to_number(value),
        applied: to_number(applied),
        reason,
    });
    applied
}
