//! Затвор: нажатие кнопки и клавиши громкости, отсчёт таймера, две фазы завершения снимка (захват и
//! сохранение, решение 140), сроки, миниатюра.

use shell_contract::clock::MonotonicNanos;
use shell_contract::haptics::HapticPattern;

use super::Ctx;
use crate::action::KeyPhase;
use crate::effect::Effect;
use crate::facts::CameraPresence;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::shutter::{CaptureOutcome, ShotRequest, ShotRequestId, ShutterState};
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

/// Кнопка затвора отвергается только под шторкой меню; при открытой карточке (`RaisedAboveCard`)
/// затвор поднят, виден и снимает (решение 103).
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
    // Решение 140: ждать сохранения могут не больше `max_saving_shots` снимков; нажатие сверх — явный отказ
    // «подожди» (и без запуска таймера: отсчёт, за которым не будет снимка, только обманул бы пользователя).
    if !ctx.state.saving.has_room() {
        return ActionOutcome::Rejected(Rejection::SavingQueueFull);
    }
    match ctx.config.timer.seconds_for(ctx.state.settings.timer) {
        None => request_capture(ctx, at),
        Some(seconds) => {
            ctx.state.shutter = ShutterState::Countdown {
                ends_at: deadline_after_seconds(at, seconds),
            };
        }
    }
    ActionOutcome::Applied
}

/// Выдаёт номер, переводит затвор в `Capturing` и просит снимок с настройками на момент `pressed_at`
/// (метка тапа/клавиши или `Tick`, на котором сработал таймер) вместе со звуком затвора.
pub(super) fn request_capture(ctx: &mut Ctx<'_>, pressed_at: MonotonicNanos) {
    let request = ctx.state.allocate_shot_request();
    ctx.state.shutter = ShutterState::Capturing { request };
    ctx.state.capture_deadline = Some(deadline_after(pressed_at, ctx.config.capture.capture_timeout_ms));
    // Момент нажатия нужен позже, когда снимок сохранится: замер «нажатие → картинка».
    ctx.state.thumbnail_tracker.note_press(request, pressed_at);
    let shot = ShotRequest {
        request,
        pressed_at,
        framing: ctx.state.zoom_framing(),
        orientation: ctx.state.orientation,
        primary_format: ctx.config.capture.primary_format,
        flash: ctx.state.effective_flash(),
        raw: ctx.state.settings.raw,
        aspect: ctx.state.settings.aspect,
    };
    ctx.emit(Effect::Capture(shot));
    // Звук идёт вместе с запросом снимка: при таймере — в момент срабатывания, не при нажатии кнопки.
    ctx.emit(Effect::PlayShutterSound);
}

/// Фаза 1: кадр захвачен. Затвор свободен (можно снимать снова), снимок ждёт сохранения.
pub(super) fn on_capture_secured(ctx: &mut Ctx<'_>, request: ShotRequestId) -> ActionOutcome {
    if !is_awaited(&ctx.state.shutter, request) {
        return ActionOutcome::Rejected(Rejection::StaleCaptureResult);
    }
    ctx.state.shutter = ShutterState::Idle;
    ctx.state.capture_deadline = None;
    let saving_deadline = deadline_after(ctx.state.last_time, ctx.config.capture.saving_timeout_ms);
    if ctx.state.saving.begin(request, saving_deadline).is_err() {
        // Недостижимо при исправной работе (нажатие при полном наборе отвергается заранее): снимок не будет
        // отслежен, его итог получит `StaleCaptureResult`. Но потеря считается, а не глотается.
        ctx.state.saving_overflows = ctx.state.saving_overflows.saturating_add(1);
    }
    ActionOutcome::Applied
}

/// Фаза 2: итог снимка. Приходит и для снимка в `Capturing` (ошибка до захвата освобождает затвор), и для
/// уже захваченного, сохраняемого (затвор при этом не трогается: он может снимать следующий).
pub(super) fn on_capture_finished(ctx: &mut Ctx<'_>, request: ShotRequestId, outcome: CaptureOutcome) -> ActionOutcome {
    if is_awaited(&ctx.state.shutter, request) {
        ctx.state.shutter = ShutterState::Idle;
        ctx.state.capture_deadline = None;
    } else if !ctx.state.saving.finish(request) {
        return ActionOutcome::Rejected(Rejection::StaleCaptureResult);
    }
    match outcome {
        // Миниатюра — последний СОХРАНЁННЫЙ по порядку прихода итогов, а не по порядку нажатий.
        CaptureOutcome::Saved => {
            let state = &mut *ctx.state;
            state.thumbnail = state.thumbnail_tracker.on_saved(request, &state.thumbnail);
        }
        CaptureOutcome::Failed(_) => {
            // Прежняя миниатюра остаётся как была: проваленный снимок картинки не ставит.
            ctx.state.thumbnail_tracker.on_failed(request);
            ctx.emit(Effect::Haptic(HapticPattern::Error));
        }
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
    // Набор сохраняемых мог не успеть освободиться: честный отказ с хаптикой, а не молчаливое «ничего».
    if !ctx.state.saving.has_room() {
        ctx.state.shutter = ShutterState::Idle;
        ctx.state.rejections.record(Rejection::SavingQueueFull);
        ctx.emit(Effect::Haptic(HapticPattern::Error));
        return;
    }
    // Момент снимка — момент срабатывания таймера, а не нажатия кнопки.
    request_capture(ctx, now);
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

/// Захваченные снимки, чей срок сохранения вышел, объявляются потерянными: считаются, пользователь получает
/// хаптику ошибки (один раз за тик). Затвор не трогается. Поздний `CaptureFinished` получит
/// `StaleCaptureResult`. Вызывается из `Tick`.
pub(super) fn expire_overdue_saving(ctx: &mut Ctx<'_>, now: MonotonicNanos) {
    let expired = ctx.state.saving.expire(now);
    if expired == 0 {
        return;
    }
    let expired = u32::try_from(expired).unwrap_or(u32::MAX);
    ctx.state.lost_captures = ctx.state.lost_captures.saturating_add(expired);
    ctx.emit(Effect::Haptic(HapticPattern::Error));
}

/// Отмена отсчёта (камера пропала).
pub(super) fn cancel_countdown(state: &mut ScreenState) {
    if state.shutter.is_counting_down() {
        state.shutter = ShutterState::Idle;
    }
}
