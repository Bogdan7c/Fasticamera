//! Связь шкал EV («солнце» у рамки фокуса и ползунок панели) с состоянием экрана.
//!
//! **Один владелец значения — `ScreenState`**: он хранит шаги EV и пределы, шкала отвечает только за физику
//! (ведение за пальцем, прилипание, хаптику тиков). Отсюда два правила:
//! * шкала → экран: событие `ValueChanged` становится `ExposureDragged` **только пока значение двигает палец**
//!   ([`FingerDrive`]); движение, начатое программно (`ExposureRestored`, сброс, отказ редьюсера), эхом не уходит;
//! * экран → шкала: если значение шкалы разошлось с состоянием и палец её не держит, шкала едет к состоянию.

use camera_contract::control::ExposureBias;
use ui_kit::scale::ScaleEvent;
use ui_kit::value::{LinearLimits, QuantizedLinear, ReferenceValue};

use super::error::{WidgetError, element};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::exposure::ExposureLimits;

/// Кто сейчас двигает шкалу: палец (события превращаются в действия) или программа (события гасятся).
#[derive(Debug, Default)]
pub(crate) struct FingerDrive {
    finger_driven: bool,
    /// Сколько событий значения погашено как эхо программной установки (для диагностики и тестов).
    suppressed_echo_events: u64,
    /// Сколько значений шкалы не удалось превратить в `ExposureBias` (NaN/∞ от шкалы): действие не выдано, но
    /// ошибка не глотается молча — она видна в диагностике.
    invalid_bias_values: u64,
}

impl FingerDrive {
    /// Палец взял шкалу (`Began` пана).
    pub(crate) fn finger_took_over(&mut self) {
        self.finger_driven = true;
    }

    /// Программа взяла шкалу: дальнейшие события значения — эхо.
    pub(crate) fn program_took_over(&mut self) {
        self.finger_driven = false;
    }

    pub(crate) fn is_finger_driven(&self) -> bool {
        self.finger_driven
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.suppressed_echo_events
    }

    pub(crate) fn invalid_bias_values(&self) -> u64 {
        self.invalid_bias_values
    }

    /// События шкалы → действия экрана и хаптика.
    pub(crate) fn translate(&mut self, events: Vec<ScaleEvent<f64>>, ctx: &Ctx<'_>, out: &mut Outputs) {
        for event in events {
            match event {
                ScaleEvent::ValueChanged { value } if self.finger_driven => match ExposureBias::from_ev(value as f32) {
                    Ok(requested) => out
                        .actions
                        .push(ScreenAction::ExposureDragged { requested, at: ctx.now }),
                    Err(_rejected) => self.invalid_bias_values += 1,
                },
                ScaleEvent::ValueChanged { .. } => self.suppressed_echo_events += 1,
                ScaleEvent::Haptic { pattern } => out.haptics.push(pattern),
                ScaleEvent::Settled { .. } => self.finger_driven = false,
                ScaleEvent::TickCrossed { .. } | ScaleEvent::ReferenceCrossed { .. } => {}
            }
        }
    }
}

/// Пределы шкалы по пределам состояния (шаги → EV).
pub(crate) fn linear_limits(ctx: &Ctx<'_>) -> LinearLimits {
    let step = f64::from(ctx.config.exposure.step_ev);
    let limits: ExposureLimits = ctx.state.exposure().limits();
    LinearLimits {
        min: f64::from(limits.min_steps()) * step,
        max: f64::from(limits.max_steps()) * step,
    }
}

/// Модель значений EV: шаг из конфига, «ноль» — опорное значение.
pub(crate) fn exposure_model(ctx: &Ctx<'_>) -> Result<QuantizedLinear, WidgetError> {
    QuantizedLinear::new(
        f64::from(ctx.config.exposure.step_ev),
        ReferenceValue::At(0.0),
        linear_limits(ctx),
    )
    .map_err(element("модель EV"))
}

/// Значение состояния в EV.
pub(crate) fn state_value_ev(ctx: &Ctx<'_>) -> f64 {
    f64::from(ctx.state.exposure().steps()) * f64::from(ctx.config.exposure.step_ev)
}

/// Разошлась ли шкала с состоянием (допуск — доля шага из конфига `state_match_tolerance_steps`).
pub(crate) fn differs_from_state(ctx: &Ctx<'_>, scale_value_ev: f64) -> bool {
    (scale_value_ev - state_value_ev(ctx)).abs()
        > f64::from(ctx.config.exposure.step_ev) * ctx.geometry.config().exposure_panel.state_match_tolerance_steps
}

/// Подпись значения EV: `0`, `+1.0`, `-0.7` (табличные цифры задаёт роль текста).
pub(crate) fn format_ev(ev: f32) -> String {
    if ev.abs() < 0.05 {
        "0".to_owned()
    } else {
        format!("{ev:+.1}")
    }
}
