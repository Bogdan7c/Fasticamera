//! Колёса цвета в разделе калибровки: выбор вида колонки и колеса, толчок и кольцо (решения 204–207).
//!
//! Инварианты те же, что у ручек (`calibration.rs`):
//! * действия допустимы только в режиме с разделом калибровки;
//! * значения колёс берутся из `LookFacts::current` — экран их не хранит; намерения ТОЧЕЧНЫЕ (`SetWheelPush`,
//!   `SetWheelRing`): один палец тянет точку, другой — кольцо или другое колесо, и ни одно намерение не несёт
//!   устаревшую копию соседнего значения;
//! * то же значение, что уже в фактах, — `NoChange(ValueUnchanged)`, наружу ничего не уходит.

use look_contract::{LookIntent, LookKnob, WheelPush, WheelZone};

use super::Ctx;
use super::calibration::require_calibration;
use super::look::{Availability, emit_look_from_facts};
use crate::calibration::ControlViewKind;
use crate::outcome::{ActionOutcome, NoChangeReason};

/// Выбран вид колонки (шкалы или колёса). Само устройство колонки пересоберёт виджет по ключу раздела.
pub(super) fn on_control_view_selected(ctx: &mut Ctx<'_>, view: ControlViewKind) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    if ctx.state.calibration.control_view() == view {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    ctx.state.calibration.set_control_view(view);
    ActionOutcome::Applied
}

/// Список шкал успокоился: запомнить его смещение (виджет начнёт с него после пересборки или возврата на SCALES).
/// Список успокоенным смещением вне `0…∞` не бывает (диапазон прокрутки — от нуля); нечисловое или отрицательное
/// значение — ошибка отправителя: в отладке падаем, в бою состояние не трогаем (`NoChange`), а не прячем в ноль.
pub(super) fn on_scales_scroll_remembered(ctx: &mut Ctx<'_>, offset_pt: f64) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    let is_valid = offset_pt.is_finite() && offset_pt >= 0.0;
    debug_assert!(
        is_valid,
        "смещение списка шкал должно быть конечным и не отрицательным: {offset_pt}"
    );
    if !is_valid {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    if ctx.state.calibration.scales_scroll_offset_pt() == offset_pt {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    ctx.state.calibration.set_scales_scroll_offset_pt(offset_pt);
    ActionOutcome::Applied
}

/// Выбрано колесо (вкладка). Повторный выбор того же — `NoChange`.
pub(super) fn on_wheel_selected(ctx: &mut Ctx<'_>, zone: WheelZone) -> ActionOutcome {
    if let Err(outcome) = require_calibration(ctx) {
        return outcome;
    }
    if ctx.state.calibration.selected_wheel() == zone {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    ctx.state.calibration.set_selected_wheel(zone);
    ActionOutcome::Applied
}

/// Точка толчка потянута: `SetWheelPush`, если она отличается от точки в фактах.
pub(super) fn on_wheel_push_dragged(ctx: &mut Ctx<'_>, zone: WheelZone, push: WheelPush) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if facts.current.settings.wheels.wheel(zone).push == push {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SetWheelPush { zone, push })
    })
}

/// Кольцо яркости потянуто: `SetWheelRing`, если значение отличается от значения в фактах.
pub(super) fn on_wheel_ring_dragged(ctx: &mut Ctx<'_>, zone: WheelZone, ring: LookKnob) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if facts.current.settings.wheels.wheel(zone).ring == ring {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SetWheelRing { zone, ring })
    })
}
