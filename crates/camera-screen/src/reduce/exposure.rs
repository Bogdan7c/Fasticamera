//! Компенсация экспозиции: «солнце», ползунок панели, сброс, программная установка.
//!
//! Камере уходит `SetExposureBias` только по действиям пользователя. [`on_restored`] эффекта не
//! выдаёт — иначе установка значения «из фактов» вернулась бы в камеру эхом. Это синхронизация
//! шкалы со значением, уже действующим в камере; если значение пришло из сохранённых настроек,
//! камере его отправляет приложение (см. `ScreenAction::ExposureRestored`).

use camera_contract::control::ExposureBias;
use shell_contract::clock::MonotonicNanos;

use super::Ctx;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::exposure::ExposureChange;
use crate::facts::CameraPresence;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::state::PanelState;

/// Перетаскивание: значение квантуется к шагу 1/3 EV и пределам.
pub(super) fn on_drag(ctx: &mut Ctx<'_>, requested: ExposureBias, at: MonotonicNanos) -> ActionOutcome {
    if let Some(rejection) = check_adjustable(ctx) {
        return ActionOutcome::Rejected(rejection);
    }
    let steps = ctx.state.exposure.quantize(requested, &ctx.config.exposure);
    if ctx.state.exposure.set_by_user(steps) == ExposureChange::Unchanged {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    // Пока палец двигает «солнце», квадрат фокуса не должен гаснуть.
    ctx.state.focus.keep_alive(at);
    send_current_bias(ctx);
    ActionOutcome::Applied
}

/// Кнопка сброса панели: значение 0.
pub(super) fn on_reset(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if let Some(rejection) = check_adjustable(ctx) {
        return ActionOutcome::Rejected(rejection);
    }
    if ctx.state.exposure.set_by_user(0) == ExposureChange::Unchanged {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    send_current_bias(ctx);
    ActionOutcome::Applied
}

/// Синхронизация шкалы с действующим значением камеры: значение меняется, камере ничего не уходит.
pub(super) fn on_restored(ctx: &mut Ctx<'_>, bias: ExposureBias) -> ActionOutcome {
    ctx.state.exposure.restore(bias, &ctx.config.exposure);
    ActionOutcome::Applied
}

/// Кнопка «×» панели экспозиции.
pub(super) fn on_panel_close(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if ctx.state.panel != PanelState::ExposurePanel {
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    ctx.state.panel = PanelState::Closed;
    ActionOutcome::Applied
}

/// Отправляет камере текущее значение (используют и обработчик фактов, когда диапазон сузился).
pub(super) fn send_current_bias(ctx: &mut Ctx<'_>) {
    let bias = ctx.state.exposure.bias(&ctx.config.exposure);
    ctx.emit(Effect::Camera(ScreenCameraIntent::SetExposureBias(bias)));
}

fn check_adjustable(ctx: &Ctx<'_>) -> Option<Rejection> {
    if ctx.state.facts.presence != CameraPresence::Live {
        return Some(Rejection::CameraNotLive);
    }
    if !ctx.state.exposure.limits().is_adjustable() {
        return Some(Rejection::ExposureUnsupported);
    }
    None
}
