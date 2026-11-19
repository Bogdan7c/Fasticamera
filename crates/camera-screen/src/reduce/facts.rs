//! Факты камеры: присутствие, точка интереса, блокировка AE/AF, прогресс фокуса, диапазон EV.
//!
//! Экран отражает то, что говорит камера; единственное исключение — ожидание подтверждения
//! (`awaited_point`, `LockState::Pending`): ответ камеры приходит позже тапа, и устаревший
//! факт «весь кадр» не должен гасить свежий квадрат.

use camera_contract::three_a::{AeAfLock, PointOfInterest};

use super::{Ctx, exposure, shutter};
use crate::facts::{CameraFacts, CameraPresence, FocusPhase};
use crate::focus::{CameraPointMatch, FocusIndicator, FocusOrigin, LockState};
use crate::outcome::ActionOutcome;

pub(super) fn on_facts_changed(ctx: &mut Ctx<'_>, facts: CameraFacts) -> ActionOutcome {
    let previous = ctx.state.facts;
    ctx.state.facts = facts;
    apply_exposure_range(ctx, &previous, &facts);
    if facts.presence == CameraPresence::Live {
        apply_live_facts(ctx, &facts);
    } else {
        apply_not_live(ctx);
    }
    ActionOutcome::Applied
}

/// Камеры нет (стартует или недоступна): индикатор и плашка скрыты, отсчёт таймера отменён,
/// ждущий снимок объявлен потерянным.
fn apply_not_live(ctx: &mut Ctx<'_>) {
    ctx.state.focus.hide();
    ctx.state.focus.lock = LockState::Unlocked;
    ctx.state.focus.phase = FocusPhase::Continuous;
    ctx.state.focus.last_seen_request = None;
    shutter::cancel_countdown(ctx.state);
    // Идущий снимок без камеры не завершится: итог уже не придёт, затвор нельзя оставлять занятым.
    shutter::abandon_capture_if_waiting(ctx);
}

fn apply_live_facts(ctx: &mut Ctx<'_>, facts: &CameraFacts) {
    ctx.state.focus.phase = facts.focus;
    // Точка раньше блокировки: подтверждение блокировки зависит от того, дошла ли новая точка.
    apply_point_of_interest(ctx, facts);
    apply_lock(ctx, facts.lock);
}

/// Диапазон компенсации учитывается всегда, даже пока камера стартует: к моменту `Live` пределы
/// шкалы уже верные. Камере же новое значение шлём только живой.
fn apply_exposure_range(ctx: &mut Ctx<'_>, previous: &CameraFacts, facts: &CameraFacts) {
    if previous.exposure_range == facts.exposure_range {
        return;
    }
    let value_moved = ctx
        .state
        .exposure
        .apply_camera_range(&ctx.config.exposure, facts.exposure_range);
    // Диапазон камеры сузился и сдвинул значение: камере надо сказать новое, иначе разойдёмся.
    if value_moved && facts.presence == CameraPresence::Live {
        exposure::send_current_bias(ctx);
    }
}

/// Блокировка: плашка видна, пока камера говорит `Locked`. `Unlocked` во время ожидания
/// подтверждения — устаревший факт, ожидание не снимает (его снимет таймаут).
///
/// `Locked` во время ожидания подтверждает блокировку только вместе с точкой новой блокировки:
/// при повторном долгом тапе на уже заблокированном кадре следующий снимок фактов может ещё нести
/// старое `Locked` от прежней блокировки — оно ничего не подтверждает.
fn apply_lock(ctx: &mut Ctx<'_>, lock: AeAfLock) {
    let focus = &mut ctx.state.focus;
    match (lock, focus.lock) {
        (AeAfLock::Locked, LockState::Pending { .. }) if focus.is_awaiting_camera_point() => {}
        (AeAfLock::Locked, _) => focus.lock = LockState::Locked,
        (AeAfLock::Unlocked, LockState::Locked) => focus.lock = LockState::Unlocked,
        (AeAfLock::Unlocked, LockState::Pending { .. } | LockState::Unlocked) => {}
    }
}

/// Квадрат фокуса ставится по точке камеры, а не по последнему тапу.
fn apply_point_of_interest(ctx: &mut Ctx<'_>, facts: &CameraFacts) {
    match facts.point_of_interest {
        PointOfInterest::WholeFrame => {
            // Свежий тап ещё не дошёл до камеры — не гасим его устаревшим «весь кадр».
            if !ctx.state.focus.is_awaiting_camera_point() {
                ctx.state.focus.hide();
                ctx.state.focus.last_seen_request = None;
            }
        }
        PointOfInterest::Point { region, request } => {
            if ctx.state.focus.last_seen_request == Some(request) {
                return;
            }
            match ctx.state.focus.match_camera_point(region.center()) {
                // Ответ на более ранний тап: номер не запоминаем, квадрат не двигаем.
                CameraPointMatch::StaleForLastTap => return,
                CameraPointMatch::AnswersLastTap => ctx.state.focus.confirm_camera_point(),
                CameraPointMatch::NotAwaiting => {}
            }
            ctx.state.focus.last_seen_request = Some(request);
            place_indicator_at_camera_point(ctx, region.center());
        }
    }
}

fn place_indicator_at_camera_point(ctx: &mut Ctx<'_>, center: camera_contract::three_a::SensorPoint) {
    // Без геометрии потока перевести нечего: оставляем индикатор как есть (до следующих фактов).
    let Ok(mapping) = ctx.state.viewfinder_mapping() else {
        return;
    };
    match mapping.sensor_to_screen(center) {
        Ok(screen_point) => match ctx.state.focus.indicator {
            FocusIndicator::Shown { .. } => ctx.state.focus.reposition(screen_point),
            FocusIndicator::Hidden => {
                let now = ctx.state.last_time;
                ctx.state.focus.show(screen_point, FocusOrigin::Camera, now);
            }
        },
        // Точка камеры вне видимого кадра (другое соотношение сторон): показывать нечего.
        Err(_) => ctx.state.focus.hide(),
    }
}
