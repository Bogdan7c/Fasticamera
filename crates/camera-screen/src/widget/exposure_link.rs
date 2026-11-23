//! Связь шкал EV («солнце» у рамки фокуса и ползунок панели) с состоянием экрана.
//!
//! **Один владелец значения — `ScreenState`**: он хранит шаги EV и пределы, шкала отвечает только за физику
//! (ведение за пальцем, прилипание, хаптику тиков). Отсюда два правила:
//! * шкала → экран: событие `ValueChanged` становится `ExposureDragged` **только пока значение двигает палец**
//!   ([`FingerDrive`]); движение, начатое программно (`ExposureRestored`, сброс, отказ редьюсера), эхом не уходит;
//! * экран → шкала: если значение шкалы разошлось с состоянием и палец её не держит, шкала едет к состоянию.

use camera_contract::control::ExposureBias;
use ui_kit::ruler::{FingerGate, GatedEvent};
use ui_kit::scale::ScaleEvent;
use ui_kit::value::{LinearLimits, QuantizedLinear, ReferenceValue};

use super::error::{WidgetError, element};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::exposure::ExposureLimits;

/// Кто сейчас двигает шкалу: палец (события превращаются в действия) или программа (события гасятся).
///
/// Правило «эхо не уходит в действия» живёт в `ui_kit::ruler::FingerGate` (общее с ручками раздела калибровки);
/// здесь только перевод значения шкалы в `ExposureBias` и счётчик значений, которые им стать не смогли.
#[derive(Debug, Default)]
pub(crate) struct FingerDrive {
    gate: FingerGate,
    /// Сколько значений шкалы не удалось превратить в `ExposureBias` (NaN/∞ от шкалы): действие не выдано, но
    /// ошибка не глотается молча — она видна в диагностике.
    invalid_bias_values: u64,
}

impl FingerDrive {
    /// Палец взял шкалу (`Began` пана).
    pub(crate) fn finger_took_over(&mut self) {
        self.gate.finger_took_over();
    }

    /// Программа взяла шкалу: дальнейшие события значения — эхо.
    pub(crate) fn program_took_over(&mut self) {
        self.gate.program_took_over();
    }

    pub(crate) fn is_finger_driven(&self) -> bool {
        self.gate.is_finger_driven()
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.gate.suppressed_echo_events()
    }

    pub(crate) fn invalid_bias_values(&self) -> u64 {
        self.invalid_bias_values
    }

    /// События шкалы → действия экрана и хаптика.
    pub(crate) fn translate(&mut self, events: Vec<ScaleEvent<f64>>, ctx: &Ctx<'_>, out: &mut Outputs) {
        let routed = self.gate.route(events);
        self.apply_routed(routed, ctx, out);
    }

    /// Уже отфильтрованные затвором события (их отдаёт `RulerControl`) → действия экрана и хаптика.
    pub(crate) fn apply_routed(&mut self, routed: Vec<GatedEvent<f64>>, ctx: &Ctx<'_>, out: &mut Outputs) {
        for event in routed {
            match event {
                GatedEvent::FingerValue(value) => match ExposureBias::from_ev(value as f32) {
                    Ok(requested) => out
                        .actions
                        .push(ScreenAction::ExposureDragged { requested, at: ctx.now }),
                    Err(_rejected) => self.invalid_bias_values += 1,
                },
                GatedEvent::Haptic(pattern) => out.haptics.push(pattern),
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
        > f64::from(ctx.config.exposure.step_ev) * ctx.geometry.config().card.state_match_tolerance_steps
}

/// Подпись значения EV в карточке iOS 27: `0.0`, `1.7`, `−2.0` (точка, минус U+2212, без «+»)
/// `[measured: C t=54.0]`. Ноль без знака: «−0.0» не бывает (значение округляется до десятых).
///
/// EV в состоянии всегда конечен: его даёт `ExposureState::bias` как `шаги × step_ev`, где шаги — целое, а
/// `step_ev` проверен конфигом на конечность; `ExposureBias::from_ev` отвергает NaN/inf. Поэтому NaN/inf сюда
/// попасть не может, а если попал — это ошибка вызывающего: в отладке падаем, а не печатаем молча «0.0»
/// (`as i32` превращает NaN в 0).
pub(crate) fn format_ev(ev: f32) -> String {
    debug_assert!(ev.is_finite(), "EV в подписи должен быть конечным, получено {ev}");
    let tenths = (ev * 10.0).round() as i32;
    let sign = if tenths < 0 { "\u{2212}" } else { "" };
    format!("{sign}{}.{}", tenths.abs() / 10, tenths.abs() % 10)
}
