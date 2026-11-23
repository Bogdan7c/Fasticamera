//! Вид картинки и пресеты: общий путь «факты от библиотеки → намерение наружу» (решения 178, 195, 196).
//!
//! Инварианты модуля:
//! * редьюсер НЕ хранит значения ручек: любое намерение собирается из `LookFacts::current`, присланного
//!   библиотекой; своих копий нет, расхождения быть не может;
//! * что можно, а что нельзя по сути (имя занято, встроенный не удалить) знает библиотека и отвечает проблемой
//!   в следующих фактах; экран отказывает только в том, что видит сам (нет фактов, нет такого пресета, не тот режим).

use look_contract::{LookFacts, LookIntent};

use super::Ctx;
use crate::effect::Effect;
use crate::outcome::{ActionOutcome, Rejection};

/// В каких режимах действие допустимо.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Availability {
    /// В любом режиме (выбор пресета в карточке Styles живёт и в PHOTO).
    AnyMode,
    /// Только в режиме с разделом калибровки.
    CalibrationOnly,
}

/// Намерение по виду «как есть» (прямой путь, без фактов): уходит наружу ровно одним эффектом.
pub(super) fn on_look(ctx: &mut Ctx<'_>, intent: LookIntent) -> ActionOutcome {
    ctx.emit(Effect::Look(intent));
    ActionOutcome::Applied
}

/// Читает факты о виде, проверив режим и наличие фактов, и достаёт из них нужное. `Err` — готовый итог
/// (отказ или «менять нечего»).
pub(super) fn read_facts<T>(
    ctx: &Ctx<'_>,
    availability: Availability,
    read: impl FnOnce(&LookFacts) -> Result<T, ActionOutcome>,
) -> Result<T, ActionOutcome> {
    if availability == Availability::CalibrationOnly && !ctx.state.mode_behaviour().has_calibration_surface() {
        return Err(ActionOutcome::Rejected(Rejection::NotInCalibrationMode));
    }
    let facts = ctx
        .state
        .look
        .facts()
        .ok_or(ActionOutcome::Rejected(Rejection::LookFactsMissing))?;
    read(facts)
}

/// Собирает намерение из фактов и отправляет его наружу одним эффектом.
pub(super) fn emit_look_from_facts(
    ctx: &mut Ctx<'_>,
    availability: Availability,
    plan: impl FnOnce(&LookFacts) -> Result<LookIntent, ActionOutcome>,
) -> ActionOutcome {
    match read_facts(ctx, availability, plan) {
        Ok(intent) => {
            ctx.emit(Effect::Look(intent));
            ActionOutcome::Applied
        }
        Err(outcome) => outcome,
    }
}

/// Новый снимок фактов: плашка проблемы, выбор полосы и взведённое удаление подстраиваются под него.
pub(super) fn on_facts_changed(ctx: &mut Ctx<'_>, facts: LookFacts) -> ActionOutcome {
    ctx.state.look.store_facts(facts);
    super::calibration::reselect_band_after_facts(ctx);
    super::presets::disarm_delete_of_vanished_preset(ctx);
    ActionOutcome::Applied
}

/// Плашка проблемы с номером `number` закрыта. Убирается только плашка с ЭТИМ номером: если в том же кадре уже
/// пришла новая проблема, закрытие старой её не трогает (такое закрытие — `NoChange`).
pub(super) fn on_problem_dismissed(ctx: &mut Ctx<'_>, number: u64) -> ActionOutcome {
    if ctx.state.look.dismiss_banner(number) {
        ActionOutcome::Applied
    } else {
        ActionOutcome::NoChange(crate::outcome::NoChangeReason::NothingToShow)
    }
}
