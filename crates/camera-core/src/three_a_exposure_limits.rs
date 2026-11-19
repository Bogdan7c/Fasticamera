//! Общие пределы ручной экспозиции: диапазоны камеры, предел длительностью кадра и поправки
//! «значение подтянуто». Один источник правды для вендорского AE и нашего (`three_a_own`):
//! полностью ручной план у обоих **обязан** быть одним и тем же, иначе переключатель A/B
//! композиции менял бы ручную съёмку.

use camera_contract::auto_exposure::ProductReach;
use camera_contract::capabilities::{
    CameraCapabilities, ExposureTimeRange, FrameDuration, IsoRange, ManualSensorRanges,
};
use camera_contract::control::{ExposureTime, Iso};
use std::time::Duration;

use crate::command::UnsupportedThreeA;
use crate::request_controls::ExposurePlan;
use crate::three_a_control::{ExposureDecision, ThreeAContext};
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter};

/// Диапазоны ручного сенсора камеры; `None` — камера ручного сенсора не умеет.
pub(crate) fn manual_sensor_ranges(capabilities: &CameraCapabilities) -> Option<(IsoRange, ExposureTimeRange)> {
    match capabilities.manual_sensor {
        ManualSensorRanges::Supported { iso, exposure_time, .. } => Some((iso, exposure_time)),
        ManualSensorRanges::NotSupported => None,
    }
}

/// Обе величины вручную: ISO и выдержка внутри диапазона камеры, выдержка не длиннее кадра.
pub(crate) fn plan_manual_sensor(
    context: &ThreeAContext<'_>,
    iso: Iso,
    shutter: ExposureTime,
) -> Result<ExposureDecision, UnsupportedThreeA> {
    let Some((iso_range, shutter_range)) = manual_sensor_ranges(context.capabilities) else {
        return Err(UnsupportedThreeA::ManualSensor);
    };
    let mut adjustments = Vec::new();
    let applied_iso = limit_manual_iso(iso, iso_range, &mut adjustments);
    let applied_shutter = limit_manual_shutter(shutter, shutter_range, context.stream_frame_duration, &mut adjustments);
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
        reach: ProductReach::Unbounded,
    })
}

/// ISO внутри диапазона камеры; подтянутое значение записано в `adjustments`.
pub(crate) fn limit_manual_iso(iso: Iso, range: IsoRange, adjustments: &mut Vec<ControlAdjustment>) -> u32 {
    clamp_with_note(
        iso.value(),
        range.min,
        range.max,
        ControlParameter::Iso,
        f64::from,
        adjustments,
    )
}

/// Ручная выдержка внутри диапазона камеры и не длиннее кадра потока (60 к/с держим только
/// короткой выдержкой); каждая подтяжка записана в `adjustments` со своей причиной.
pub(crate) fn limit_manual_shutter(
    shutter: ExposureTime,
    range: ExposureTimeRange,
    stream_frame_duration: FrameDuration,
    adjustments: &mut Vec<ControlAdjustment>,
) -> Duration {
    let mut applied_shutter = clamp_with_note(
        shutter.duration(),
        range.shortest,
        range.longest,
        ControlParameter::Shutter,
        |value| value.as_nanos() as f64,
        adjustments,
    );
    let frame_limit = frame_limit(stream_frame_duration, range.shortest);
    if let Some(limit) = frame_limit.filter(|limit| applied_shutter > *limit) {
        adjustments.push(ControlAdjustment {
            parameter: ControlParameter::Shutter,
            requested: shutter.duration().as_nanos() as f64,
            applied: limit.as_nanos() as f64,
            reason: AdjustmentReason::LimitedByFrameDuration,
        });
        applied_shutter = limit;
    }
    applied_shutter
}

/// Самая длинная выдержка, которую можно просить на этом потоке: меньшее из предела камеры
/// и длительности кадра.
pub(crate) fn longest_usable_shutter(range: ExposureTimeRange, stream_frame_duration: FrameDuration) -> Duration {
    frame_limit(stream_frame_duration, range.shortest).map_or(range.longest, |limit| limit.min(range.longest))
}

/// Предел выдержки по длительности кадра; `None`, если кадр нулевой (HAL не ограничил) или
/// короче минимальной выдержки камеры (предел бессмыслен).
pub(crate) fn frame_limit(frame: FrameDuration, shortest: Duration) -> Option<Duration> {
    let limit = frame.as_duration();
    (limit >= shortest && !limit.is_zero()).then_some(limit)
}

/// Подтянуть значение к диапазону; если подтянули — записать, что и почему.
pub(crate) fn clamp_with_note<T: PartialOrd + Copy>(
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
