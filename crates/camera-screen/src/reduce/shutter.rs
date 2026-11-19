//! Затвор: нажатие кнопки и клавиши громкости, отсчёт таймера, итог снимка, миниатюра.

use shell_contract::clock::MonotonicNanos;
use shell_contract::haptics::HapticPattern;

use super::Ctx;
use crate::action::KeyPhase;
use crate::effect::Effect;
use crate::facts::CameraPresence;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::shutter::{CaptureOutcome, ShotRequest, ShotRequestId, ShutterState, ThumbnailState};
use crate::state::{ScreenState, ShutterVisibility};
use crate::time::{deadline_after, deadline_after_seconds, has_reached};

/// Кто нажал затвор. Отличается одним: кнопку на экране закрывает шторка меню, клавишу — нет (решение 94).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutterSource {
    OnScreenButton,
    VolumeKey,
}

/// Тап по кнопке затвора на экране.
pub(super) fn on_shutter_button(ctx: &mut Ctx<'_>, at: MonotonicNanos) -> ActionOutcome {
    press(ctx, ShutterSource::OnScreenButton, at)
}

/// Клавиша громкости: снимает и при открытом меню; отпускание и автоповтор — без действия
/// (место под удержание: QuickTake, серия).
pub(super) fn on_volume_key(ctx: &mut Ctx<'_>, phase: KeyPhase, at: MonotonicNanos) -> ActionOutcome {
    match phase {
        KeyPhase::Pressed => press(ctx, ShutterSource::VolumeKey, at),
        KeyPhase::Released | KeyPhase::Repeated => ActionOutcome::NoChange(NoChangeReason::KeyHoldNotSupported),
    }
}

fn press(ctx: &mut Ctx<'_>, source: ShutterSource, at: MonotonicNanos) -> ActionOutcome {
    if source == ShutterSource::OnScreenButton && ctx.state.shutter_visibility() == ShutterVisibility::HiddenUnderDrawer
    {
        return ActionOutcome::Rejected(Rejection::ShutterHiddenByMenu);
    }
    match ctx.state.shutter {
        // Нажатие во время отсчёта отменяет его.
        ShutterState::Countdown { .. } => {
            ctx.state.shutter = ShutterState::Idle;
            ActionOutcome::Applied
        }
        ShutterState::Idle => start_from_idle(ctx, at),
        // Идёт съёмка или запись: второй запрос не рождаем, отказ считается.
        ShutterState::Capturing { .. }
        | ShutterState::CapturingLong { .. }
        | ShutterState::Burst { .. }
        | ShutterState::Recording { .. }
        | ShutterState::RecordingLocked { .. } => ActionOutcome::Rejected(Rejection::ShutterBusy),
    }
}

fn start_from_idle(ctx: &mut Ctx<'_>, at: MonotonicNanos) -> ActionOutcome {
    if ctx.state.facts.presence != CameraPresence::Live {
        return ActionOutcome::Rejected(Rejection::CameraNotLive);
    }
    match ctx.config.timer.seconds_for(ctx.state.settings.timer) {
        None => request_capture(ctx),
        Some(seconds) => {
            ctx.state.shutter = ShutterState::Countdown {
                ends_at: deadline_after_seconds(at, seconds),
            };
        }
    }
    ActionOutcome::Applied
}

/// Выдаёт номер, переводит затвор в `Capturing` и просит снимок с настройками на этот момент.
pub(super) fn request_capture(ctx: &mut Ctx<'_>) {
    let request = ctx.state.allocate_shot_request();
    ctx.state.shutter = ShutterState::Capturing { request };
    // Метка времени у этого пути всегда свежая: тап/клавиша или `Tick`, который и вызвал отсчёт.
    ctx.state.capture_deadline = Some(deadline_after(
        ctx.state.last_time,
        ctx.config.capture.result_timeout_ms,
    ));
    let settings = ctx.state.settings;
    ctx.emit(Effect::Capture(ShotRequest {
        request,
        flash: settings.flash,
        raw: settings.raw,
        aspect: settings.aspect,
    }));
}

/// Итог снимка от сервиса съёмки.
pub(super) fn on_capture_finished(ctx: &mut Ctx<'_>, request: ShotRequestId, outcome: CaptureOutcome) -> ActionOutcome {
    if !is_awaited(&ctx.state.shutter, request) {
        return ActionOutcome::Rejected(Rejection::StaleCaptureResult);
    }
    ctx.state.shutter = ShutterState::Idle;
    ctx.state.capture_deadline = None;
    match outcome {
        CaptureOutcome::Saved => ctx.state.thumbnail = ThumbnailState::Last { shot: request },
        CaptureOutcome::Failed(_) => ctx.emit(Effect::Haptic(HapticPattern::Error)),
    }
    ActionOutcome::Applied
}

fn is_awaited(shutter: &ShutterState, request: ShotRequestId) -> bool {
    match shutter {
        ShutterState::Capturing { request: awaited }
        | ShutterState::CapturingLong { request: awaited, .. }
        | ShutterState::Burst { request: awaited, .. } => *awaited == request,
        ShutterState::Idle
        | ShutterState::Countdown { .. }
        | ShutterState::Recording { .. }
        | ShutterState::RecordingLocked { .. } => false,
    }
}

/// Тап по миниатюре: открыть системную галерею (она есть и без снимков этой сессии).
pub(super) fn on_thumbnail_tapped(ctx: &mut Ctx<'_>) -> ActionOutcome {
    ctx.emit(Effect::OpenSystemGallery);
    ActionOutcome::Applied
}

/// Срок таймера: если пришло время — просим снимок. Вызывается из `Tick`.
pub(super) fn fire_countdown_if_due(ctx: &mut Ctx<'_>, now: MonotonicNanos) {
    let ShutterState::Countdown { ends_at } = ctx.state.shutter else {
        return;
    };
    if now < ends_at {
        return;
    }
    // Камера могла пропасть во время отсчёта: обработчик фактов отменяет отсчёт сам, но на всякий
    // случай не просим снимок у неработающей камеры и честно считаем отказ.
    if ctx.state.facts.presence != CameraPresence::Live {
        ctx.state.shutter = ShutterState::Idle;
        ctx.state.rejections.record(Rejection::CameraNotLive);
        return;
    }
    request_capture(ctx);
}

/// Снимок объявляется потерянным: затвор свободен, потеря посчитана, пользователь получает
/// хаптику ошибки (как при `CaptureOutcome::Failed`). Поздний `CaptureFinished` после этого
/// получит отказ `StaleCaptureResult`, а не воскресит снимок.
fn abandon_capture(ctx: &mut Ctx<'_>) {
    ctx.state.shutter = ShutterState::Idle;
    ctx.state.capture_deadline = None;
    ctx.state.lost_captures = ctx.state.lost_captures.saturating_add(1);
    ctx.emit(Effect::Haptic(HapticPattern::Error));
}

/// Камера пропала (стартует заново или недоступна): сервис снимка итога не пришлёт — освобождаем затвор.
pub(super) fn abandon_capture_if_waiting(ctx: &mut Ctx<'_>) {
    if matches!(ctx.state.shutter, ShutterState::Capturing { .. }) {
        abandon_capture(ctx);
    }
}

/// Итог снимка не пришёл в срок (сервис снимка умер при живой камере). Вызывается из `Tick`.
pub(super) fn abandon_capture_if_overdue(ctx: &mut Ctx<'_>, now: MonotonicNanos) {
    let overdue = matches!(ctx.state.shutter, ShutterState::Capturing { .. })
        && ctx
            .state
            .capture_deadline
            .is_some_and(|deadline| has_reached(now, deadline));
    if overdue {
        abandon_capture(ctx);
    }
}

/// Отмена отсчёта (камера пропала).
pub(super) fn cancel_countdown(state: &mut ScreenState) {
    if state.shutter.is_counting_down() {
        state.shutter = ShutterState::Idle;
    }
}
