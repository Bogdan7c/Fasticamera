//! Видоискатель: тап (фокус и экспозиция в точке) и долгий тап (блокировка AE/AF).

use shell_contract::clock::MonotonicNanos;
use shell_contract::haptics::HapticPattern;
use ui_core::geometry::Point;

use camera_contract::three_a::SensorPoint;

use super::Ctx;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::CameraPresence;
use crate::focus::{FocusOrigin, LockState};
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::state::{PanelState, ViewfinderUnavailable};
use crate::viewfinder_map::MappingError;

/// Результат перевода точки экрана в точку сенсора для жеста.
enum TapTarget {
    /// Точка сенсора, в которую надо навести автоматику.
    Sensor(SensorPoint),
    /// Жест не годится: итог уже решён.
    Done(ActionOutcome),
}

/// Тап по видоискателю.
///
/// * Панель или меню открыты — тап лишь закрывает их, фокус не трогаем (`behavior.md` 2.6, решение 94).
/// * Иначе — квадрат фокуса в точку тапа и `FocusAndExposeAt`. Команда сама снимает блокировку AE/AF
///   (док `CameraCommand::FocusAndExposeAt`), поэтому отдельный сброс не нужен; плашка «LOCK»
///   исчезнет, когда камера подтвердит `Unlocked`.
pub(super) fn on_tap(ctx: &mut Ctx<'_>, point: Point, at: MonotonicNanos) -> ActionOutcome {
    if ctx.state.panel != PanelState::Closed {
        ctx.state.panel = PanelState::Closed;
        return ActionOutcome::Applied;
    }
    let sensor = match locate_target(ctx, point) {
        TapTarget::Sensor(sensor) => sensor,
        TapTarget::Done(outcome) => return outcome,
    };
    ctx.state.focus.show(point, FocusOrigin::Tap, at);
    ctx.state.focus.await_camera_point(sensor);
    // Тап отменяет ожидание блокировки: камера снимет её сама вместе с новой точкой.
    if matches!(ctx.state.focus.lock, LockState::Pending { .. }) {
        ctx.state.focus.lock = LockState::Unlocked;
    }
    ctx.emit(Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(sensor)));
    ActionOutcome::Applied
}

/// Долгий тап: блокировка AE/AF в точке. Плашка появится только по факту камеры `Locked`.
pub(super) fn on_long_press(ctx: &mut Ctx<'_>, point: Point, at: MonotonicNanos) -> ActionOutcome {
    if ctx.state.panel != PanelState::Closed {
        return ActionOutcome::NoChange(NoChangeReason::PanelOpen);
    }
    let sensor = match locate_target(ctx, point) {
        TapTarget::Sensor(sensor) => sensor,
        TapTarget::Done(outcome) => return outcome,
    };
    ctx.state.focus.show(point, FocusOrigin::Lock, at);
    ctx.state.focus.await_camera_point(sensor);
    ctx.state
        .focus
        .begin_lock_wait(at, ctx.config.focus.lock_confirmation_timeout_ms);
    ctx.emit(Effect::Camera(ScreenCameraIntent::LockFocusAndExposureAt(sensor)));
    // Хаптика срабатывания блокировки — по таблице `haptics.md` (impact medium).
    ctx.emit(Effect::Haptic(HapticPattern::MediumImpact));
    ActionOutcome::Applied
}

/// Проверяет, что жест можно превратить в наведение автоматики, и переводит точку в сенсор.
fn locate_target(ctx: &Ctx<'_>, point: Point) -> TapTarget {
    if ctx.state.facts.presence != CameraPresence::Live {
        return TapTarget::Done(ActionOutcome::Rejected(Rejection::CameraNotLive));
    }
    let mapping = match ctx.state.viewfinder_mapping() {
        Ok(mapping) => mapping,
        Err(ViewfinderUnavailable::NoLayout) => {
            return TapTarget::Done(ActionOutcome::Rejected(Rejection::InvalidSurface));
        }
        Err(ViewfinderUnavailable::NoStreamGeometry) => {
            return TapTarget::Done(ActionOutcome::Rejected(Rejection::StreamGeometryUnknown));
        }
        Err(ViewfinderUnavailable::Mapping(_)) => {
            return TapTarget::Done(ActionOutcome::Rejected(Rejection::ViewfinderMappingFailed));
        }
    };
    match mapping.screen_to_sensor(point) {
        Ok(sensor) => TapTarget::Sensor(sensor),
        // Тап по чёрной полосе вне кадра — не ошибка, а «фокус не ставим».
        Err(MappingError::OutsideFrame) => TapTarget::Done(ActionOutcome::NoChange(NoChangeReason::TapOutsideFrame)),
        Err(_) => TapTarget::Done(ActionOutcome::Rejected(Rejection::ViewfinderMappingFailed)),
    }
}
