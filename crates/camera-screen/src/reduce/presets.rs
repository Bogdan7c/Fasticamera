//! Пресеты: выбор, сброс правок, сохранение, переименование, удаление и ввод имени (решения 187, 195–197, 201).
//!
//! Инварианты модуля:
//! * экран только шлёт намерения; что имя занято или пресет встроенный, решает библиотека (ответ — проблема в
//!   следующих фактах); экран отказывает лишь в том, что видит сам (нет фактов, нет такого пресета, не тот режим);
//! * ввод имени — машина `TextFlow`: `Idle → AwaitingId → Pending → Idle` (при отказе отправки `AwaitingId` возвращается к прежнему `Pending`, если он был). Ответ с чужим номером
//!   (вытесненный, повторный, пришедший без запроса) — `NoChange`, а не ошибка и не действие;
//! * удаление требует двух тапов по одному пресету (`DeleteConfirmation`), любое другое пользовательское действие
//!   снимает взвод. Это придумано (двойного тапа у Apple нет): удаление необратимо, а кнопка рядом с «Save».

use look_contract::{EditState, LookIntent, PresetId, PresetKind, PresetName, UserPresetId};
use shell_contract::text_input::{TextInputOutcome, TextInputResult, TextPurpose, TextRequestId};

use super::Ctx;
use super::look::{Availability, emit_look_from_facts, read_facts};
use crate::action::ScreenAction;
use crate::calibration::{DeleteConfirmation, Naming, PendingText, TextFlow};
use crate::effect::Effect;
use crate::look_view::ProblemKind;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::state::ScreenState;

/// Выбор пресета (ряд имён раздела или карточка Styles в PHOTO). Повторный тап по активному пресету без правок —
/// `NoChange`; с правками — выбор возвращает пресет к исходным значениям.
pub(super) fn on_preset_selected(ctx: &mut Ctx<'_>, preset: PresetId) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::AnyMode, |facts| {
        if !facts.presets.iter().any(|entry| entry.id == preset) {
            return Err(ActionOutcome::Rejected(Rejection::UnknownPreset));
        }
        if facts.active == preset && facts.edits == EditState::Clean {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SelectPreset(preset))
    })
}

/// «↻» карточки Styles / Reset меню пресетов: отбросить правки поверх пресета. Правок нет — `NoChange` (тусклая «↻»).
pub(super) fn on_discard_edits(ctx: &mut Ctx<'_>) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::AnyMode, |facts| match facts.edits {
        EditState::Clean => Err(ActionOutcome::NoChange(NoChangeReason::NoEditsOnTop)),
        EditState::EditedOnTop => Ok(LookIntent::DiscardEdits),
    })
}

/// Save: записать текущий вид в активный СВОЙ пресет. Встроенный — отказ; правок нет — `NoChange`.
pub(super) fn on_overwrite_preset(ctx: &mut Ctx<'_>) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        let PresetId::User(user_preset) = facts.active else {
            return Err(ActionOutcome::Rejected(Rejection::BuiltinPresetReadOnly));
        };
        if facts.edits == EditState::Clean {
            return Err(ActionOutcome::NoChange(NoChangeReason::NoEditsOnTop));
        }
        Ok(LookIntent::OverwritePreset(user_preset))
    })
}

/// New: спросить имя. Поле открывается ПУСТЫМ: имя активного пресета заняло бы место нового (библиотека
/// отвергает повтор имени), а переименовать есть отдельная кнопка.
pub(super) fn on_create_preset(ctx: &mut Ctx<'_>) -> ActionOutcome {
    match read_facts(ctx, Availability::CalibrationOnly, |_| Ok(())) {
        Ok(()) => request_name(ctx, Naming::NewPreset, TextPurpose::NewPresetName, String::new()),
        Err(outcome) => outcome,
    }
}

/// Rename: спросить новое имя своего пресета; поле открывается с его нынешним именем.
pub(super) fn on_rename_preset(ctx: &mut Ctx<'_>, preset: UserPresetId) -> ActionOutcome {
    let current_name = read_facts(ctx, Availability::CalibrationOnly, |facts| {
        find_user_preset_name(facts.presets.iter(), preset).ok_or(ActionOutcome::Rejected(Rejection::UnknownPreset))
    });
    match current_name {
        Ok(name) => request_name(ctx, Naming::Rename(preset), TextPurpose::RenamePreset, name),
        Err(outcome) => outcome,
    }
}

fn find_user_preset_name<'a>(
    mut presets: impl Iterator<Item = &'a look_contract::PresetEntry>,
    preset: UserPresetId,
) -> Option<String> {
    presets
        .find(|entry| entry.id == PresetId::User(preset) && entry.kind == PresetKind::User)
        .map(|entry| entry.name.as_str().to_owned())
}

/// Ставит `AwaitingId` и просит у приложения диалог. Принятый брокером новый запрос вытесняет прежний (старый
/// получит `Superseded`, его номер мы перестанем ждать на `TextRequestIssued`); отказанный — нет, и тогда
/// прежний диалог восстанавливается в `on_text_request_not_sent`.
fn request_name(ctx: &mut Ctx<'_>, naming: Naming, purpose: TextPurpose, initial_text: String) -> ActionOutcome {
    // Открытый диалог не бросаем: если новый запрос не уйдёт, он останется единственным живым (см. `TextFlow`).
    let previous_dialog = match ctx.state.calibration.text_flow() {
        TextFlow::Pending(pending) => Some(pending),
        TextFlow::AwaitingId { previous_dialog, .. } => previous_dialog,
        TextFlow::Idle => None,
    };
    ctx.state.calibration.set_text_flow(TextFlow::AwaitingId {
        naming,
        previous_dialog,
    });
    ctx.emit(Effect::RequestText { purpose, initial_text });
    ActionOutcome::Applied
}

/// Приложение выдало номер запросу: `AwaitingId → Pending`. Номер без ожидания — устаревшая обратная связь.
pub(super) fn on_text_request_issued(ctx: &mut Ctx<'_>, id: TextRequestId) -> ActionOutcome {
    let TextFlow::AwaitingId { naming, .. } = ctx.state.calibration.text_flow() else {
        return ActionOutcome::NoChange(NoChangeReason::StaleTextResult);
    };
    ctx.state
        .calibration
        .set_text_flow(TextFlow::Pending(PendingText { id, naming }));
    ActionOutcome::Applied
}

/// Запрос отправить не удалось: диалога не будет, ждать нечего, пользователь видит проблему.
pub(super) fn on_text_request_not_sent(ctx: &mut Ctx<'_>) -> ActionOutcome {
    let TextFlow::AwaitingId { previous_dialog, .. } = ctx.state.calibration.text_flow() else {
        return ActionOutcome::NoChange(NoChangeReason::StaleTextResult);
    };
    // Прежний диалог (если был) остаётся открытым у брокера: возвращаем его, иначе введённое имя потеряется.
    ctx.state.calibration.set_text_flow(match previous_dialog {
        Some(pending) => TextFlow::Pending(pending),
        None => TextFlow::Idle,
    });
    ctx.state.look.raise_local_problem(ProblemKind::KeyboardUnavailable);
    ActionOutcome::Applied
}

/// Ответ клавиатуры. Сверяется с ожидаемым номером; `Entered` проходит проверку имени экрана.
///
/// Ожидаемых номера может быть два: пока новый запрос ждёт номер (`AwaitingId`), прежний диалог ещё жив, и его
/// ответ принимается как обычный (иначе введённое имя пропало бы).
pub(super) fn on_text_input_finished(ctx: &mut Ctx<'_>, result: TextInputResult) -> ActionOutcome {
    let (answered, next_flow) = match ctx.state.calibration.text_flow() {
        TextFlow::Pending(pending) if pending.id == result.id => (pending, TextFlow::Idle),
        TextFlow::AwaitingId {
            naming,
            previous_dialog: Some(previous),
        } if previous.id == result.id => (
            previous,
            TextFlow::AwaitingId {
                naming,
                previous_dialog: None,
            },
        ),
        _ => return ActionOutcome::NoChange(NoChangeReason::StaleTextResult),
    };
    ctx.state.calibration.set_text_flow(next_flow);
    match result.outcome {
        TextInputOutcome::Entered(text) => apply_entered_name(ctx, answered.naming, &text),
        // Отказ владельца или вытеснение — не проблема: диалога нет, правок нет.
        TextInputOutcome::Cancelled | TextInputOutcome::Superseded => {}
        TextInputOutcome::Unavailable => ctx.state.look.raise_local_problem(ProblemKind::KeyboardUnavailable),
    }
    ActionOutcome::Applied
}

/// Проверяет введённое имя и шлёт намерение; плохое имя — локальная проблема экрана, намерения нет.
fn apply_entered_name(ctx: &mut Ctx<'_>, naming: Naming, text: &str) {
    match PresetName::new(text) {
        Ok(name) => ctx.emit(Effect::Look(match naming {
            Naming::NewPreset => LookIntent::SaveAsNewPreset(name),
            Naming::Rename(preset) => LookIntent::RenamePreset { preset, name },
        })),
        Err(error) => ctx
            .state
            .look
            .raise_local_problem(ProblemKind::InvalidPresetName(error)),
    }
}

/// Delete: первый тап взводит, второй по тому же пресету удаляет.
pub(super) fn on_delete_preset(ctx: &mut Ctx<'_>, preset: UserPresetId) -> ActionOutcome {
    let exists = read_facts(ctx, Availability::CalibrationOnly, |facts| {
        Ok(find_user_preset_name(facts.presets.iter(), preset).is_some())
    });
    match exists {
        Err(outcome) => outcome,
        Ok(false) => ActionOutcome::Rejected(Rejection::UnknownPreset),
        Ok(true) if ctx.state.calibration.delete_confirmation() == DeleteConfirmation::Armed(preset) => {
            ctx.state.calibration.set_delete_confirmation(DeleteConfirmation::Idle);
            ctx.emit(Effect::Look(LookIntent::DeletePreset(preset)));
            ActionOutcome::Applied
        }
        Ok(true) => {
            ctx.state
                .calibration
                .set_delete_confirmation(DeleteConfirmation::Armed(preset));
            ActionOutcome::Applied
        }
    }
}

/// Надо ли снять взвод удаления из-за этого действия. Любое действие пользователя, кроме подтверждающего тапа по
/// тому же пресету, снимает взвод — в том числе отказанное или «ничего не изменившее»: нажатие на что-то другое
/// отменяет намерение удалить. Фоновые действия (время, датчики, факты, итоги снимка) взвод не трогают: иначе он
/// слетал бы на ближайшем `Tick`.
pub(super) fn should_disarm_delete(state: &ScreenState, action: &ScreenAction) -> bool {
    let DeleteConfirmation::Armed(armed) = state.calibration().delete_confirmation() else {
        return false;
    };
    if is_background(action) {
        return false;
    }
    !matches!(action, ScreenAction::DeletePresetTapped(tapped) if *tapped == armed)
}

/// Не жест пользователя: приходит сам.
fn is_background(action: &ScreenAction) -> bool {
    matches!(
        action,
        ScreenAction::Tick(_)
            | ScreenAction::GravityChanged(_)
            | ScreenAction::CameraFactsChanged(_)
            | ScreenAction::LookFactsChanged(_)
            | ScreenAction::LookStartupProblem(_)
            | ScreenAction::LookWhiteStopsChanged(_)
            | ScreenAction::DeviceOrientationChanged(_)
            | ScreenAction::SurfaceResized(_)
            | ScreenAction::SafeAreaChanged(_)
            | ScreenAction::CaptureSecured { .. }
            | ScreenAction::CaptureFinished { .. }
            | ScreenAction::TextRequestIssued { .. }
            | ScreenAction::TextRequestNotSent
            | ScreenAction::TextInputFinished(_)
            | ScreenAction::ScopeDemandLost
            | ScreenAction::ProblemDismissed { .. }
    )
}

/// Удалённый (или исчезнувший из списка) пресет не может оставаться взведённым.
pub(super) fn disarm_delete_of_vanished_preset(ctx: &mut Ctx<'_>) {
    let DeleteConfirmation::Armed(armed) = ctx.state.calibration.delete_confirmation() else {
        return;
    };
    let still_listed = ctx
        .state
        .look
        .facts()
        .is_some_and(|facts| find_user_preset_name(facts.presets.iter(), armed).is_some());
    if !still_listed {
        ctx.state.calibration.set_delete_confirmation(DeleteConfirmation::Idle);
    }
}
