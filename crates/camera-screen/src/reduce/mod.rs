//! Редьюсер экрана: чистая функция `(состояние, действие, конфиг) → (новое состояние, эффекты, итог)`.
//!
//! Без часов, ввода-вывода и случайности: один и тот же вход даёт один и тот же выход. Обработчики
//! разбиты по темам: видоискатель и фокус, меню и настройки, затвор и таймер, факты камеры,
//! экспозиция, время и датчики.
//!
//! # Инварианты
//! * Отказ ([`ActionOutcome::Rejected`]) не меняет состояние, кроме счётчика отказов и метки
//!   последнего известного времени (время идёт и при отказе), и не выдаёт эффектов. Единственное
//!   исключение — обработчик, который явно вызвал [`Ctx::keep_changes_despite_rejection`]
//!   (невалидный размер surface сбрасывает раскладку).
//! * Исключение второе: любое пользовательское действие, кроме подтверждающего тапа Delete, снимает взвод удаления
//!   пресета даже при отказе и при итоге `NoChange` (отмена намерения «удалить» — не изменение значений экрана).
//! * Входное состояние никогда не изменяется (работаем с копией).
//! * Действие, которое ничего не изменило (не считая метки времени) и не выдало эффектов,
//!   получает итог `NoChange`: `Tick` без видимых изменений — тоже `NoChange`.

mod calibration;
mod exposure;
mod facts;
mod look;
mod menu;
mod mode_change;
mod mode_guard;
mod presets;
mod scopes;
mod sensors;
mod shutter;
mod viewfinder;
mod wheels;
mod white;
mod zoom;

#[cfg(test)]
#[path = "calibration_mode_tests.rs"]
mod calibration_mode_tests;
#[cfg(test)]
#[path = "calibration_tests.rs"]
mod calibration_tests;
#[cfg(test)]
#[path = "card_tests.rs"]
mod card_tests;
#[cfg(test)]
#[path = "exposure_tests.rs"]
mod exposure_tests;
#[cfg(test)]
#[path = "facts_tests.rs"]
mod facts_tests;
#[cfg(test)]
#[path = "flash_tests.rs"]
mod flash_tests;
#[cfg(test)]
#[path = "look_tests.rs"]
mod look_tests;
#[cfg(test)]
#[path = "menu_tests.rs"]
mod menu_tests;
#[cfg(test)]
#[path = "mode_guard_tests.rs"]
mod mode_guard_tests;
#[cfg(test)]
#[path = "presets_tests.rs"]
mod presets_tests;
#[cfg(test)]
#[path = "saving_tests.rs"]
mod saving_tests;
#[cfg(test)]
#[path = "scopes_tests.rs"]
mod scopes_tests;
#[cfg(test)]
#[path = "sensors_tests.rs"]
mod sensors_tests;
#[cfg(test)]
#[path = "shutter_tests.rs"]
mod shutter_tests;
#[cfg(test)]
#[path = "standby_tests.rs"]
mod standby_tests;
#[cfg(test)]
#[path = "thumbnail_tests.rs"]
mod thumbnail_tests;
#[cfg(test)]
#[path = "viewfinder_tests.rs"]
mod viewfinder_tests;
#[cfg(test)]
#[path = "wheels_tests.rs"]
mod wheels_tests;
#[cfg(test)]
#[path = "white_tests.rs"]
mod white_tests;
#[cfg(test)]
#[path = "zoom_gesture_tests.rs"]
mod zoom_gesture_tests;
#[cfg(test)]
#[path = "zoom_tests.rs"]
mod zoom_tests;

use shell_contract::clock::MonotonicNanos;

use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::Effect;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::state::ScreenState;

/// Результат применения действия.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenTransition {
    /// Новое состояние.
    pub state: ScreenState,
    /// Намерения наружу, в порядке исполнения.
    pub effects: Vec<Effect>,
    /// Явный итог действия.
    pub outcome: ActionOutcome,
}

/// Рабочий контекст обработчиков: изменяемая копия состояния, список эффектов и конфиг.
pub(crate) struct Ctx<'a> {
    pub(crate) state: &'a mut ScreenState,
    pub(crate) effects: &'a mut Vec<Effect>,
    pub(crate) config: &'a ScreenConfig,
    /// Выставляется обработчиком, чьи изменения должны пережить отказ.
    keeps_changes_on_rejection: bool,
}

impl Ctx<'_> {
    /// Изменения этого действия сохраняются, даже если итог — отказ (эффекты всё равно отбрасываются).
    /// Нужен, когда отказ сам по себе делает прежнее состояние неверным, а не просто «не применяется».
    pub(crate) fn keep_changes_despite_rejection(&mut self) {
        self.keeps_changes_on_rejection = true;
    }

    pub(crate) fn emit(&mut self, effect: Effect) {
        self.effects.push(effect);
    }
}

/// Применяет действие к состоянию.
pub fn reduce(state: &ScreenState, action: ScreenAction, config: &ScreenConfig) -> ScreenTransition {
    let mut next = state.clone();
    if let Some(moment) = moment_of(&action) {
        next.last_time = next.last_time.max(moment);
    }
    // Взвод удаления снимается ДО обработчика (он увидит `Idle`) и заново ПОСЛЕ отказа: отказ откатывает рабочую
    // копию, но отменённое намерение «удалить» не должно воскреснуть от неудачного тапа по чему-то другому.
    let disarm_delete = presets::should_disarm_delete(state, &action);
    if disarm_delete {
        next.calibration
            .set_delete_confirmation(crate::calibration::DeleteConfirmation::Idle);
    }
    let mut effects = Vec::new();
    let (outcome, keeps_changes_on_rejection) = {
        let mut ctx = Ctx {
            state: &mut next,
            effects: &mut effects,
            config,
            keeps_changes_on_rejection: false,
        };
        let outcome = dispatch(&mut ctx, action);
        (outcome, ctx.keeps_changes_on_rejection)
    };
    match outcome {
        ActionOutcome::Rejected(reason) if keeps_changes_on_rejection => {
            next.rejections.record(reason);
            ScreenTransition {
                state: next,
                effects: Vec::new(),
                outcome,
            }
        }
        ActionOutcome::Rejected(reason) => {
            let mut rejected = reject(state, reason, next.last_time);
            if disarm_delete {
                rejected
                    .state
                    .calibration
                    .set_delete_confirmation(crate::calibration::DeleteConfirmation::Idle);
            }
            rejected
        }
        ActionOutcome::Applied if effects.is_empty() && is_unchanged_apart_from_time(state, &mut next) => {
            ScreenTransition {
                state: next,
                effects,
                outcome: ActionOutcome::NoChange(NoChangeReason::ValueUnchanged),
            }
        }
        outcome => ScreenTransition {
            state: next,
            effects,
            outcome,
        },
    }
}

/// Совпадает ли `next` со старым состоянием, не считая метки времени (она продвигается каждым
/// `Tick` и сама по себе изменением экрана не является).
fn is_unchanged_apart_from_time(state: &ScreenState, next: &mut ScreenState) -> bool {
    let advanced_time = next.last_time;
    next.last_time = state.last_time;
    let unchanged = *next == *state;
    next.last_time = advanced_time;
    unchanged
}

/// Отказ: состояние как было, плюс счётчик причины и продвинутое время (иначе индикатор,
/// созданный позже по факту камеры, получил бы устаревшее `since`).
fn reject(state: &ScreenState, reason: Rejection, advanced_time: MonotonicNanos) -> ScreenTransition {
    let mut unchanged = state.clone();
    unchanged.last_time = advanced_time;
    unchanged.rejections.record(reason);
    ScreenTransition {
        state: unchanged,
        effects: Vec::new(),
        outcome: ActionOutcome::Rejected(reason),
    }
}

/// Момент времени, который несёт действие (если несёт).
fn moment_of(action: &ScreenAction) -> Option<MonotonicNanos> {
    match action {
        ScreenAction::ViewfinderTapped { at, .. }
        | ScreenAction::ViewfinderLongPressed { at, .. }
        | ScreenAction::ExposureDragged { at, .. }
        | ScreenAction::FocusSunHeld { at }
        | ScreenAction::ShutterTapped { at }
        | ScreenAction::VolumeKey { at, .. }
        | ScreenAction::ZoomStopTapped { at, .. }
        | ScreenAction::ZoomStripPressBegan { at }
        | ScreenAction::ZoomStripPressEnded { at }
        | ScreenAction::ZoomWheelTouched { at }
        | ScreenAction::ZoomWheelReleased { at }
        | ScreenAction::ZoomWheelDragged { at, .. }
        | ScreenAction::ZoomPinchBegan { at }
        | ScreenAction::ZoomPinchChanged { at, .. }
        | ScreenAction::ZoomPinchEnded { at }
        | ScreenAction::FlipCameraRequested { at } => Some(*at),
        ScreenAction::Tick(now) => Some(*now),
        _ => None,
    }
}

/// Применяет действие, а затем сверяет спрос на тёплого соседа: любое действие могло открыть колесо, начать
/// щипок или закрыть их, а переход спроса обязан дойти до движка (см. `zoom::sync_standby_demand`).
/// Отказ состояние не меняет, поэтому сверять после него нечего.
fn dispatch(ctx: &mut Ctx<'_>, action: ScreenAction) -> ActionOutcome {
    let outcome = dispatch_action(ctx, action);
    if !matches!(outcome, ActionOutcome::Rejected(_)) {
        zoom::sync_standby_demand(ctx);
        // Режим, поворот, размер surface и выбор прибора вместе решают, какой запрос прибора нужен рендеру.
        calibration::sync_after_action(ctx);
    }
    outcome
}

fn dispatch_action(ctx: &mut Ctx<'_>, action: ScreenAction) -> ActionOutcome {
    if let Err(rejected) = mode_guard::require_camera_controls(ctx, &action) {
        return rejected;
    }
    match action {
        ScreenAction::ViewfinderTapped { point, at } => viewfinder::on_tap(ctx, point, at),
        ScreenAction::ViewfinderLongPressed { point, at } => viewfinder::on_long_press(ctx, point, at),
        ScreenAction::ExposureDragged { requested, at } => exposure::on_drag(ctx, requested, at),
        ScreenAction::FocusSunHeld { at } => exposure::on_sun_held(ctx, at),
        ScreenAction::ExposureResetRequested => exposure::on_reset(ctx),
        ScreenAction::ExposureRestored { bias } => exposure::on_restored(ctx, bias),
        ScreenAction::ExposurePanelCloseRequested => exposure::on_panel_close(ctx),
        ScreenAction::ModeSelected(mode) => menu::on_mode_selected(ctx, mode),
        ScreenAction::ControlsMenuOpenRequested => menu::on_open(ctx),
        ScreenAction::ControlsMenuCloseRequested => menu::on_close(ctx),
        ScreenAction::ControlsItemSelected(tool) => menu::on_item_selected(ctx, tool),
        ScreenAction::SubmenuOptionSelected(option) => menu::on_option_selected(ctx, option),
        ScreenAction::SubmenuBackRequested => menu::on_submenu_back(ctx),
        ScreenAction::StripToolTapped(tool) => menu::on_strip_tool_tapped(ctx, tool),
        ScreenAction::ShutterTapped { at } => shutter::on_shutter_button(ctx, at),
        ScreenAction::VolumeKey { phase, at } => shutter::on_volume_key(ctx, phase, at),
        ScreenAction::ThumbnailTapped => shutter::on_thumbnail_tapped(ctx),
        ScreenAction::CaptureSecured { request } => shutter::on_capture_secured(ctx, request),
        ScreenAction::CaptureFinished { request, outcome } => shutter::on_capture_finished(ctx, request, outcome),
        ScreenAction::CameraFactsChanged(facts) => facts::on_facts_changed(ctx, facts),
        ScreenAction::DeviceOrientationChanged(orientation) => sensors::on_orientation(ctx, orientation),
        ScreenAction::GravityChanged(gravity) => sensors::on_gravity(ctx, gravity),
        ScreenAction::Tick(now) => sensors::on_tick(ctx, now),
        ScreenAction::SurfaceResized(size) => sensors::on_surface_resized(ctx, size),
        ScreenAction::SafeAreaChanged(insets) => sensors::on_safe_area(ctx, insets),
        ScreenAction::ZoomStopTapped { stop, at } => zoom::on_stop_tapped(ctx, stop, at),
        ScreenAction::ZoomStripPressBegan { at } => zoom::on_strip_press_began(ctx, at),
        ScreenAction::ZoomStripPressEnded { at } => zoom::on_strip_press_ended(ctx, at),
        ScreenAction::ZoomWheelTouched { at } => zoom::on_wheel_touched(ctx, at),
        ScreenAction::ZoomWheelReleased { at } => zoom::on_wheel_released(ctx, at),
        ScreenAction::ZoomWheelDragged { requested, at } => zoom::on_wheel_dragged(ctx, requested, at),
        ScreenAction::ZoomPinchBegan { at } => zoom::on_pinch_began(ctx, at),
        ScreenAction::ZoomPinchChanged { scale, at } => zoom::on_pinch_changed(ctx, scale, at),
        ScreenAction::ZoomPinchEnded { at } => zoom::on_pinch_ended(ctx, at),
        ScreenAction::FlipCameraRequested { at } => zoom::on_flip(ctx, at),
        ScreenAction::Look(intent) => look::on_look(ctx, intent),
        ScreenAction::Scopes(intent) => scopes::on_scopes(ctx, intent),
        ScreenAction::LookFactsChanged(facts) => look::on_facts_changed(ctx, *facts),
        ScreenAction::KnobDragged { knob, value } => calibration::on_knob_dragged(ctx, knob, value),
        ScreenAction::BandKnobDragged { band, axis, value } => {
            calibration::on_band_knob_dragged(ctx, band, axis, value)
        }
        ScreenAction::ControlViewSelected(view) => wheels::on_control_view_selected(ctx, view),
        ScreenAction::ScalesScrollRemembered { offset_pt } => wheels::on_scales_scroll_remembered(ctx, offset_pt),
        ScreenAction::WheelSelected(zone) => wheels::on_wheel_selected(ctx, zone),
        ScreenAction::WheelPushDragged { zone, push } => wheels::on_wheel_push_dragged(ctx, zone, push),
        ScreenAction::WheelRingDragged { zone, ring } => wheels::on_wheel_ring_dragged(ctx, zone, ring),
        ScreenAction::ToneBaseSelected(base) => calibration::on_tone_base_selected(ctx, base),
        ScreenAction::ResetControlTapped(control) => calibration::on_reset_control(ctx, control),
        ScreenAction::ScopeKindSelected(kind) => calibration::on_scope_kind_selected(ctx, kind),
        ScreenAction::VectorscopeAxesSelected(axes) => calibration::on_axes_selected(ctx, axes),
        ScreenAction::CleanViewPressed => calibration::on_clean_view_pressed(ctx),
        ScreenAction::CleanViewReleased => calibration::on_clean_view_released(ctx),
        ScreenAction::BandSelected(band) => calibration::on_band_selected(ctx, band),
        ScreenAction::PresetSelected(preset) => presets::on_preset_selected(ctx, preset),
        ScreenAction::DiscardEditsTapped => presets::on_discard_edits(ctx),
        ScreenAction::OverwritePresetTapped => presets::on_overwrite_preset(ctx),
        ScreenAction::CreatePresetTapped => presets::on_create_preset(ctx),
        ScreenAction::RenamePresetTapped(preset) => presets::on_rename_preset(ctx, preset),
        ScreenAction::DeletePresetTapped(preset) => presets::on_delete_preset(ctx, preset),
        ScreenAction::TextRequestIssued { id } => presets::on_text_request_issued(ctx, id),
        ScreenAction::TextRequestNotSent => presets::on_text_request_not_sent(ctx),
        ScreenAction::TextInputFinished(result) => presets::on_text_input_finished(ctx, result),
        ScreenAction::ProblemDismissed { number } => look::on_problem_dismissed(ctx, number),
        ScreenAction::ScopeDemandLost => calibration::on_scope_demand_lost(ctx),
        ScreenAction::WhiteButtonTapped => white::on_button_tapped(ctx),
        ScreenAction::WhiteButtonLongPressed => white::on_button_long_pressed(ctx),
        ScreenAction::WhiteAutoTapped => white::on_auto_tapped(ctx),
        ScreenAction::WhitePhoneTapped => white::on_phone_tapped(ctx),
        ScreenAction::WhiteLightSelected(index) => white::on_light_selected(ctx, index),
        ScreenAction::WhiteAxisSelected(axis) => white::on_axis_selected(ctx, axis),
        ScreenAction::WhiteTemperatureDragged(temperature) => white::on_temperature_dragged(ctx, temperature),
        ScreenAction::WhiteTintDragged(tint) => white::on_tint_dragged(ctx, tint),
        ScreenAction::WhiteTakeNotDelivered => white::on_take_not_delivered(ctx),
        ScreenAction::WhiteFollowNotDelivered => white::on_follow_not_delivered(ctx),
    }
}
