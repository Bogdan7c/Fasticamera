//! Датчики, время и размер surface: ориентация, наклон для уровня, `Tick`, раскладка.

use shell_contract::clock::MonotonicNanos;
use shell_contract::gravity::GravityVector;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;

use super::{Ctx, shutter, zoom};
use crate::effect::Effect;
use crate::layout::ScreenLayout;
use crate::level::level_reading;
use crate::outcome::{ActionOutcome, Rejection};

/// Ориентация телефона: меняется только целевой угол значков, раскладка остаётся прежней.
pub(super) fn on_orientation(ctx: &mut Ctx<'_>, orientation: DeviceOrientation) -> ActionOutcome {
    ctx.state.orientation = orientation;
    ActionOutcome::Applied
}

/// Наклон: показание уровня пересчитывается всегда (виджет решает, показывать ли по настройке).
pub(super) fn on_gravity(ctx: &mut Ctx<'_>, gravity: GravityVector) -> ActionOutcome {
    ctx.state.level = level_reading(gravity, &ctx.config.level);
    ActionOutcome::Applied
}

/// Время: отсчёт таймера, затухание индикатора, срок ожидания блокировки.
pub(super) fn on_tick(ctx: &mut Ctx<'_>, now: MonotonicNanos) -> ActionOutcome {
    shutter::fire_countdown_if_due(ctx, now);
    shutter::abandon_capture_if_overdue(ctx, now);
    ctx.state.focus.fade_if_due(now, ctx.config.focus.indicator_visible_ms);
    // Итог проверки (`LockWaitCheck`) не нужен: снятое ожидание видно по изменению состояния.
    ctx.state.focus.expire_lock_wait(now);
    zoom::on_tick(ctx, now);
    ActionOutcome::Applied
}

/// Новый размер surface: пересчёт раскладки и команда рендеру о месте видоискателя.
///
/// Невалидный размер — отказ `InvalidSurface`, но **раскладка при этом сбрасывается**: старая
/// раскладка описывает уже несуществующую поверхность, и тапы/маска по ней попадали бы мимо. Без
/// раскладки тапы по кадру получают явный отказ (`locate_target` → `InvalidSurface`), пока не придёт
/// валидный размер. Это единственное действие, чьи изменения переживают отказ.
pub(super) fn on_surface_resized(ctx: &mut Ctx<'_>, size: SurfaceSize) -> ActionOutcome {
    let Ok(layout) = ScreenLayout::new(&ctx.config.layout, size) else {
        ctx.state.surface = None;
        ctx.state.layout = None;
        ctx.keep_changes_despite_rejection();
        return ActionOutcome::Rejected(Rejection::InvalidSurface);
    };
    ctx.state.surface = Some(size);
    ctx.state.layout = Some(layout);
    if let Some(intent) = ctx.state.framing_intent() {
        ctx.emit(Effect::Viewfinder(intent));
    }
    ActionOutcome::Applied
}
