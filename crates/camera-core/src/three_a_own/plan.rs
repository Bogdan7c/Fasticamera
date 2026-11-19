//! Чистое построение плана экспозиции нашим AE: цель + компенсация + ручные якоря → `Sensor`.
//!
//! Функция не владеет состоянием и ничего не меняет: вход — принятая «основа» плана
//! ([`PlanBasis`]) и контекст движка, выход — решение. Поэтому её можно звать для проверки
//! кандидатов (`validate`) и многократно, не боясь сдвинуть следующие планы.

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::control::{Control, ExposureTime, Iso};

use super::flicker::FlickerProtection;
use super::split::{Anchor, Split, SplitLimits, split};
use crate::command::UnsupportedThreeA;
use crate::request_controls::ExposurePlan;
use crate::three_a_control::{ExposureDecision, ThreeAContext};
use crate::three_a_exposure_limits::{
    limit_manual_iso, limit_manual_shutter, longest_usable_shutter, manual_sensor_ranges, plan_manual_sensor,
};
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter};

/// Всё принятое исполнителем состояние, от которого зависит план.
#[derive(Debug, Clone, Copy)]
pub(super) struct PlanBasis {
    /// Цель без компенсации: принятое предложение, память прошлого потока или стартовое.
    pub(super) target: ExposureProduct,
    pub(super) flicker: FlickerProtection,
    /// Допуск «цель достигнута», EV.
    pub(super) not_reached_tolerance_ev: f32,
    /// Шаг округления сообщаемой нехватки, EV.
    pub(super) shortfall_report_step_ev: f32,
}

pub(super) fn plan_exposure(
    basis: &PlanBasis,
    context: &ThreeAContext<'_>,
) -> Result<ExposureDecision, UnsupportedThreeA> {
    let controls = &context.intent.controls;
    let Some((iso_range, shutter_range)) = manual_sensor_ranges(context.capabilities) else {
        // Без ручного сенсора `AE_MODE_OFF` невозможен: движок сам деградирует до вендорского авто.
        return Err(UnsupportedThreeA::ManualSensor);
    };
    // Обе величины вручную: тот же план, что у вендорского AE (общий код и те же поправки).
    if let (Control::Manual(iso), Control::Manual(shutter)) = (controls.iso, controls.shutter) {
        return plan_manual_sensor(context, iso, shutter);
    }

    let mut adjustments = Vec::new();
    // Компенсация непрерывная, без квантования шагом HAL: сенсор ведём сами.
    let requested = basis
        .target
        .scaled_by_ev(f64::from(controls.exposure_bias.ev()))
        .unwrap_or(basis.target);
    let anchor = match (controls.iso, controls.shutter) {
        (Control::Manual(iso), _) => Anchor::Iso(limit_manual_iso(iso, iso_range, &mut adjustments)),
        (_, Control::Manual(shutter)) => Anchor::Shutter(limit_manual_shutter(
            shutter,
            shutter_range,
            context.stream_frame_duration,
            &mut adjustments,
        )),
        (Control::Auto, Control::Auto) => Anchor::Free,
    };
    let limits = SplitLimits {
        iso_min: iso_range.min,
        iso_max: iso_range.max,
        shutter_shortest: shutter_range.shortest,
        shutter_cap: longest_usable_shutter(shutter_range, context.stream_frame_duration),
    };
    let divided = split(requested.seconds_iso(), anchor, limits, basis.flicker.period());
    // Нулевые границы диапазонов — нелепые характеристики камеры, а не ошибка пользователя.
    let (Ok(iso), Ok(exposure_time)) = (Iso::new(divided.iso), ExposureTime::new(divided.shutter)) else {
        return Err(UnsupportedThreeA::ManualSensor);
    };
    adjustments.extend(target_not_reached(requested, divided, basis));
    Ok(ExposureDecision {
        plan: ExposurePlan::Sensor { exposure_time, iso },
        adjustments,
    })
}

/// Поправка «цель не достигнута», если нехватка выше допуска.
///
/// Для `ControlParameter::ExposureTarget` поля **относительные, в EV от цели**: `requested`
/// всегда 0 (цель — точка отсчёта), `applied` — нехватка, округлённая до шага конфига
/// (< 0 темнее нужного, > 0 светлее). Так поправка зависит только от нехватки, а не от
/// абсолютной цели: дрожание цели у предела не плодит `ControlsAdjusted`. `applied − requested`
/// движок отдаёт рендеру как `shortfall_ev` (ошибка не больше половины шага).
fn target_not_reached(requested: ExposureProduct, divided: Split, basis: &PlanBasis) -> Option<ControlAdjustment> {
    let achieved = ExposureProduct::from_seconds_iso(divided.product_seconds_iso()).ok()?;
    let shortfall_ev = achieved.ev_above(requested);
    if shortfall_ev.abs() <= f64::from(basis.not_reached_tolerance_ev) {
        return None;
    }
    let step = f64::from(basis.shortfall_report_step_ev);
    Some(ControlAdjustment {
        parameter: ControlParameter::ExposureTarget,
        requested: 0.0,
        applied: (shortfall_ev / step).round() * step,
        reason: AdjustmentReason::TargetNotReached,
    })
}
