//! Зум: тап по кнопке ленты, колесо, щипок, переворот камеры, переход по времени, факты об объективах.
//!
//! Правило одно для всех источников значения: коэффициент зажимается в пределы группы текущего
//! направления (их задаёт каталог камер, не экран), меняется в [`crate::zoom::ZoomState`], а наружу идут два
//! эффекта — намерение объектива [`Effect::Lens`] (ящик «последнее побеждает») и пересчитанное кадрирование
//! [`Effect::Viewfinder`] (рендер и тап получают те же входы формулы, решение 93). Исключение — **синхронизация
//! по фактам** (усыновление, откат после неудачи смены, смена пределов): значение уже действует в камере, и
//! намерение обратно было бы эхом (петлёй), поэтому уходит только кадрирование.

use camera_contract::ids::LensFacing;
use camera_contract::zoom::{LensIntent, ZoomGesture, ZoomRange, ZoomRatio};
use shell_contract::clock::MonotonicNanos;

use super::Ctx;
use crate::effect::Effect;
use crate::facts::CameraFacts;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::state::PanelState;
use crate::zoom::FailureWatch;

/// Наносекунд в миллисекунде (конфиг хранит время в мс).
const NANOS_PER_MS: f64 = 1_000_000.0;

/// Пределы группы текущего направления; `None` — зума нет (нет каталога или направления).
fn current_range(ctx: &Ctx<'_>) -> Option<ZoomRange> {
    ctx.state.zoom_group().map(|group| group.range)
}

/// Намерение + кадрирование: значение изменили мы (пользователь, переворот).
///
/// `gesture` говорит движку, как выбирать камеру. **Правило жестов: одно явное действие = одно намерение.**
/// Тап по упору публикует ровно ОДНО `Discrete` с коэффициентом упора в момент тапа ([`publish_lens_intent`]);
/// последующий переход (glide) меняет только значение на экране и кадрирование ([`publish_framing`]) и новых
/// намерений не шлёт. Причина: движок на каждое `Discrete`, выбравшее заблокированный объектив, снимает блок
/// и повторяет открытие — поток промежуточных шагов дал бы несколько повторов за один тап. Переворот — одно
/// `Discrete`; колесо и щипок — `Continuous` на каждое изменение (движок применяет гистерезис и блок).
fn publish_change(ctx: &mut Ctx<'_>, gesture: ZoomGesture) {
    publish_lens_intent(ctx, ctx.state.zoom.ratio(), gesture);
    publish_framing(ctx);
}

/// Только намерение объектива с заданным коэффициентом (кадрирование — отдельно).
fn publish_lens_intent(ctx: &mut Ctx<'_>, ratio: ZoomRatio, gesture: ZoomGesture) {
    ctx.emit(Effect::Lens(LensIntent {
        facing: ctx.state.zoom.facing(),
        ratio,
        gesture,
    }));
}

/// Только кадрирование: значение пришло от камеры или определено каталогом.
fn publish_framing(ctx: &mut Ctx<'_>) {
    if let Some(intent) = ctx.state.framing_intent() {
        ctx.emit(Effect::Viewfinder(intent));
    }
}

/// Поставить `requested`, зажав в пределы. Возвращает `true`, если значение изменилось.
fn apply_ratio(ctx: &mut Ctx<'_>, range: ZoomRange, requested: ZoomRatio) -> bool {
    let clamped = range.clamp(requested);
    if clamped == ctx.state.zoom.ratio() {
        return false;
    }
    ctx.state.zoom.set_ratio(clamped);
    true
}

pub(super) fn on_stop_tapped(ctx: &mut Ctx<'_>, stop: usize, at: MonotonicNanos) -> ActionOutcome {
    let Some(group) = ctx.state.zoom_group() else {
        return ActionOutcome::Rejected(Rejection::ZoomUnavailable);
    };
    let Some(target) = group.stops.get(stop).map(|stop| stop.ratio) else {
        return ActionOutcome::Rejected(Rejection::ZoomStopMissing);
    };
    if target == ctx.state.zoom.ratio() && !ctx.state.zoom.is_gliding() {
        ctx.state.zoom.mark_user_chosen();
        return ActionOutcome::NoChange(NoChangeReason::ValueUnchanged);
    }
    ctx.state.zoom.start_glide(target, at, ctx.config.zoom.motion.glide_ms);
    // Одно намерение на тап: сразу целевой упор (новый тап посреди перехода шлёт новое — последнее побеждает).
    publish_lens_intent(ctx, target, ZoomGesture::Discrete);
    // Первый шаг перехода — сразу: кадрирование не ждёт следующего кадра.
    advance_glide(ctx, at);
    ActionOutcome::Applied
}

pub(super) fn on_wheel_touched(ctx: &mut Ctx<'_>, at: MonotonicNanos) -> ActionOutcome {
    if ctx.state.zoom_group().is_none() {
        return ActionOutcome::Rejected(Rejection::ZoomUnavailable);
    }
    ctx.state.zoom.mark_user_chosen();
    ctx.state.zoom.cancel_glide();
    ctx.state.zoom.open_wheel(at);
    ctx.state.zoom.wheel_finger(true, at);
    ActionOutcome::Applied
}

pub(super) fn on_wheel_released(ctx: &mut Ctx<'_>, at: MonotonicNanos) -> ActionOutcome {
    if !ctx.state.zoom.is_wheel_open() {
        return ActionOutcome::NoChange(NoChangeReason::NothingToClose);
    }
    ctx.state.zoom.wheel_finger(false, at);
    ActionOutcome::Applied
}

pub(super) fn on_wheel_dragged(ctx: &mut Ctx<'_>, requested: ZoomRatio, at: MonotonicNanos) -> ActionOutcome {
    let Some(range) = current_range(ctx) else {
        return ActionOutcome::Rejected(Rejection::ZoomUnavailable);
    };
    if !ctx.state.zoom.is_wheel_open() {
        // Событие запоздалой шкалы после ухода колеса: колесо закрыто, значение не трогаем.
        return ActionOutcome::NoChange(NoChangeReason::NothingToShow);
    }
    ctx.state.zoom.cancel_glide();
    ctx.state.zoom.note_wheel_activity(at);
    if apply_ratio(ctx, range, requested) {
        publish_change(ctx, ZoomGesture::Continuous);
    }
    ActionOutcome::Applied
}

pub(super) fn on_pinch_began(ctx: &mut Ctx<'_>, _at: MonotonicNanos) -> ActionOutcome {
    if ctx.state.zoom_group().is_none() {
        return ActionOutcome::Rejected(Rejection::ZoomUnavailable);
    }
    ctx.state.zoom.cancel_glide();
    ctx.state.zoom.begin_pinch();
    ActionOutcome::Applied
}

pub(super) fn on_pinch_changed(ctx: &mut Ctx<'_>, scale: f64, at: MonotonicNanos) -> ActionOutcome {
    if !scale.is_finite() || scale <= 0.0 {
        return ActionOutcome::Rejected(Rejection::InvalidZoomInput);
    }
    let Some(range) = current_range(ctx) else {
        return ActionOutcome::Rejected(Rejection::ZoomUnavailable);
    };
    let Some(start) = ctx.state.zoom.pinch_start_ratio() else {
        // Изменение без начала: арена не должна такого слать; считаем «нечего менять», не гадаем.
        return ActionOutcome::NoChange(NoChangeReason::NothingToShow);
    };
    let Ok(requested) = ZoomRatio::new((f64::from(start.value()) * scale) as f32) else {
        return ActionOutcome::Rejected(Rejection::InvalidZoomInput);
    };
    // Открытое колесо видит щипок как обычное движение значения и не гаснет под пальцами.
    ctx.state.zoom.note_wheel_activity(at);
    if apply_ratio(ctx, range, requested) {
        publish_change(ctx, ZoomGesture::Continuous);
        ActionOutcome::Applied
    } else {
        ActionOutcome::NoChange(NoChangeReason::ValueUnchanged)
    }
}

pub(super) fn on_pinch_ended(ctx: &mut Ctx<'_>, at: MonotonicNanos) -> ActionOutcome {
    if ctx.state.zoom.pinch_start_ratio().is_none() {
        return ActionOutcome::NoChange(NoChangeReason::NothingToClose);
    }
    ctx.state.zoom.end_pinch();
    ctx.state.zoom.note_wheel_activity(at);
    ActionOutcome::Applied
}

pub(super) fn on_flip(ctx: &mut Ctx<'_>, at: MonotonicNanos) -> ActionOutcome {
    let target_facing = ctx.state.flip_target_facing();
    let Some(target_group) = ctx.state.flip_target_group() else {
        return ActionOutcome::Rejected(Rejection::ZoomUnavailable);
    };
    // Возврат на заднюю — последний задний коэффициент, фронталка — «1×» (решение 115); оба зажаты в пределы.
    let wanted = match target_facing {
        LensFacing::Back => ctx.state.zoom.last_back_ratio(),
        LensFacing::Front | LensFacing::External => ZoomRatio::ONE,
    };
    let ratio = target_group.range.clamp(wanted);
    ctx.state.zoom.flip_to(target_facing, ratio, at);
    publish_change(ctx, ZoomGesture::Discrete);
    ActionOutcome::Applied
}

/// Шаг перехода к упору и уход молчащего колеса: вызывается на каждом `Tick`.
pub(super) fn on_tick(ctx: &mut Ctx<'_>, now: MonotonicNanos) {
    advance_glide(ctx, now);
    // Меню или карточка открылись поверх: колесо уходит, как и лента (значение остаётся).
    if ctx.state.panel != PanelState::Closed {
        ctx.state.zoom.close_wheel();
    }
    let idle_nanos = (ctx.config.zoom.wheel.idle_dismiss_ms * NANOS_PER_MS).round() as i64;
    ctx.state.zoom.close_wheel_if_idle(now, idle_nanos);
}

fn advance_glide(ctx: &mut Ctx<'_>, now: MonotonicNanos) {
    let Some(step) = ctx.state.zoom.glide_step(now) else {
        return;
    };
    let changed = ctx.state.zoom.ratio() != step.ratio;
    ctx.state.zoom.set_ratio(step.ratio);
    if step.finished {
        ctx.state.zoom.cancel_glide();
    }
    if changed {
        // Намерение тап уже опубликовал целиком; переход двигает только значение на экране и кадрирование.
        publish_framing(ctx);
    }
}

/// Факты об объективах: усыновление значения, откат после неудачи смены, смена пределов каталога.
/// Намерений наружу не порождает (см. модуль); кадрирование пересылает общий обработчик фактов, сравнив
/// его до и после.
pub(super) fn on_facts(ctx: &mut Ctx<'_>, previous: &CameraFacts) {
    let lens = ctx.state.facts.lens.clone();
    let failure_sequence = lens.last_failure.as_ref().map(|failure| failure.sequence);

    let active_lens = lens.active.as_ref().and_then(|active| {
        lens.catalog
            .as_ref()
            .and_then(|catalog| catalog.lens_for_camera(&active.camera))
    });

    // 1. Новая неудача смены объектива (номер больше учтённого) → откат в пределы активной камеры.
    let new_failure = match ctx.state.zoom.failure_watch {
        FailureWatch::Unarmed => false,
        FailureWatch::Armed { last_seen } => failure_sequence > last_seen,
    };
    // Откат нужен, но подогнать значение не к чему (активной камеры или её места в каталоге ещё нет): неудачу
    // нельзя «съесть» — она остаётся неучтённой и откатит значение с первыми фактами об активной камере.
    // Иначе экран навсегда застрял бы на значении, которого камера не показывает.
    let snap_pending = new_failure && ctx.state.zoom.is_adopted() && active_lens.is_none();
    if !snap_pending {
        ctx.state.zoom.failure_watch = FailureWatch::Armed {
            last_seen: match ctx.state.zoom.failure_watch {
                FailureWatch::Armed { last_seen } => last_seen.max(failure_sequence),
                FailureWatch::Unarmed => failure_sequence,
            },
        };
    }

    // 2. Усыновление: первое значение берём у камеры, а не придумываем.
    if !ctx.state.zoom.is_adopted() {
        if let Some((group, point)) = active_lens {
            ctx.state.zoom.adopt(group.facing, point.native_ratio);
        }
    } else if new_failure && let Some((group, point)) = active_lens {
        // Смена не удалась: камера осталась прежней, значение подгоняем под неё (синхронизация, без эха).
        let low = point.native_ratio.value();
        let high = point.max_ratio().value();
        let clamped = ctx.state.zoom.ratio().value().clamp(low, high);
        ctx.state.zoom.cancel_glide();
        ctx.state.zoom.end_pinch();
        let facing = group.facing;
        let snapped = ZoomRatio::new(clamped).unwrap_or(point.native_ratio);
        ctx.state.zoom.sync_facing(facing);
        ctx.state.zoom.set_ratio(snapped);
    }

    // 3. Пределы каталога сменились: значение остаётся в пределах (синхронизация, без эха).
    if previous.lens.catalog != lens.catalog
        && ctx.state.zoom.is_adopted()
        && let Some(range) = current_range(ctx)
    {
        let clamped = range.clamp(ctx.state.zoom.ratio());
        ctx.state.zoom.set_ratio(clamped);
    }
}
