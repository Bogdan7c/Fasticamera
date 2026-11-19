//! Меню Controls, подменю, значки над кадром, карусель режимов и настройки съёмки.

use super::Ctx;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::modes::CaptureMode;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::AspectRatio;
use crate::state::{PanelState, SubmenuOrigin};
use crate::tools::{ToolId, ToolOption};

/// Выбор режима. Повторный выбор текущего открывает меню Controls (`behavior.md` 2.4).
pub(super) fn on_mode_selected(ctx: &mut Ctx<'_>, mode: CaptureMode) -> ActionOutcome {
    if ctx.state.registry.descriptor(mode).is_none() {
        return ActionOutcome::Rejected(Rejection::UnknownMode);
    }
    if mode == ctx.state.mode {
        return on_open(ctx);
    }
    ctx.state.mode = mode;
    ActionOutcome::Applied
}

/// Открыть меню Controls (кнопка меню, свайп вверх, тап по текущему режиму).
pub(super) fn on_open(ctx: &mut Ctx<'_>) -> ActionOutcome {
    ctx.state.panel = PanelState::ControlsMenu;
    ActionOutcome::Applied
}

/// Закрыть меню (свайп вниз). Панель экспозиции этим жестом не закрывается.
pub(super) fn on_close(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if matches!(ctx.state.panel, PanelState::ControlsMenu | PanelState::Submenu(_)) {
        ctx.state.panel = PanelState::Closed;
    }
    ActionOutcome::Applied
}

/// Пункт меню: Exposure открывает панель, остальные — своё подменю.
pub(super) fn on_item_selected(ctx: &mut Ctx<'_>, tool: ToolId) -> ActionOutcome {
    if ctx.state.panel != PanelState::ControlsMenu || !menu_contains(ctx, tool) {
        return ActionOutcome::Rejected(Rejection::ToolNotInMenu);
    }
    ctx.state.panel = match tool {
        ToolId::Exposure => PanelState::ExposurePanel,
        other => PanelState::Submenu(other),
    };
    ctx.state.submenu_origin = SubmenuOrigin::ControlsMenu;
    ActionOutcome::Applied
}

/// Панель, в которую ведёт выход из подменю (выбор варианта или «Назад»): в меню Controls, если
/// подменю открыто из него, и в никуда, если подменю открыто значком над кадром.
fn panel_after_submenu(origin: SubmenuOrigin) -> PanelState {
    match origin {
        SubmenuOrigin::ControlsMenu => PanelState::ControlsMenu,
        SubmenuOrigin::StripIcon => PanelState::Closed,
    }
}

/// Вариант в подменю: применяется настройка, подменю закрывается (см. [`panel_after_submenu`]).
pub(super) fn on_option_selected(ctx: &mut Ctx<'_>, option: ToolOption) -> ActionOutcome {
    if ctx.state.panel != PanelState::Submenu(option.tool()) {
        return ActionOutcome::Rejected(Rejection::OptionNotInSubmenu);
    }
    if !apply_option(ctx, option) {
        return ActionOutcome::Rejected(Rejection::UnsupportedOption);
    }
    ctx.state.panel = panel_after_submenu(ctx.state.submenu_origin);
    ActionOutcome::Applied
}

/// Назад из подменю.
pub(super) fn on_submenu_back(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if matches!(ctx.state.panel, PanelState::Submenu(_)) {
        ctx.state.panel = panel_after_submenu(ctx.state.submenu_origin);
    }
    ActionOutcome::Applied
}

/// Тап по значку над кадром.
pub(super) fn on_strip_tool_tapped(ctx: &mut Ctx<'_>, tool: ToolId) -> ActionOutcome {
    if matches!(ctx.state.panel, PanelState::ControlsMenu | PanelState::Submenu(_)) {
        return ActionOutcome::Rejected(Rejection::PanelBlocksAction);
    }
    if !strip_contains(ctx, tool) {
        return ActionOutcome::Rejected(Rejection::ToolNotInStrip);
    }
    if !ctx.state.visible_strip_tools().contains(&tool) {
        return ActionOutcome::NoChange(NoChangeReason::IconNotShown);
    }
    match tool {
        ToolId::Flash => {
            // Auto не поддержан (инвариант 6): тап переключает только Off ↔ On.
            let next = ctx.state.settings.flash.toggled();
            ctx.state.settings.flash = next;
        }
        ToolId::Raw => ctx.state.settings.raw = ctx.state.settings.raw.toggled(),
        ToolId::Timer => {
            ctx.state.panel = PanelState::Submenu(ToolId::Timer);
            ctx.state.submenu_origin = SubmenuOrigin::StripIcon;
        }
        ToolId::Exposure => {
            ctx.state.panel = match ctx.state.panel {
                PanelState::ExposurePanel => PanelState::Closed,
                _ => PanelState::ExposurePanel,
            };
        }
        // ToolSet::new не пускает в ряд значков другие инструменты; ветка недостижима, но отказ явный.
        ToolId::Aspect | ToolId::Grid | ToolId::Level | ToolId::Live | ToolId::Styles | ToolId::Night => {
            return ActionOutcome::Rejected(Rejection::ToolNotInStrip);
        }
    }
    ActionOutcome::Applied
}

/// Применяет вариант к настройкам. `false` — вариант не поддержан (автовспышка).
fn apply_option(ctx: &mut Ctx<'_>, option: ToolOption) -> bool {
    match option {
        ToolOption::Flash(mode) => {
            if !mode.is_selectable() {
                return false;
            }
            ctx.state.settings.flash = mode;
        }
        ToolOption::Raw(raw) => ctx.state.settings.raw = raw,
        ToolOption::Timer(timer) => ctx.state.settings.timer = timer,
        ToolOption::Aspect(aspect) => change_aspect(ctx, aspect),
        ToolOption::Grid(grid) => ctx.state.settings.grid = grid,
        ToolOption::Level(level) => ctx.state.settings.level = level,
    }
    true
}

/// Смена соотношения: рендеру — новое кадрирование и место, индикатор фокуса скрыт.
///
/// Если квадрат был на экране, камере уходит `ResetPointOfInterest`: иначе камера продолжала бы
/// держать старую точку, а на экране её бы уже не было (экран и камера разошлись бы).
fn change_aspect(ctx: &mut Ctx<'_>, aspect: AspectRatio) {
    if ctx.state.settings.aspect == aspect {
        return;
    }
    ctx.state.settings.aspect = aspect;
    let indicator_was_shown = ctx.state.focus.indicator() != crate::focus::FocusIndicator::Hidden;
    ctx.state.focus.hide();
    if indicator_was_shown {
        ctx.emit(Effect::Camera(ScreenCameraIntent::ResetPointOfInterest));
    }
    if let Some(intent) = ctx.state.framing_intent() {
        ctx.emit(Effect::Viewfinder(intent));
    }
}

fn menu_contains(ctx: &Ctx<'_>, tool: ToolId) -> bool {
    ctx.state.menu_tools().contains(&tool)
}

fn strip_contains(ctx: &Ctx<'_>, tool: ToolId) -> bool {
    ctx.state
        .registry
        .descriptor(ctx.state.mode)
        .is_some_and(|descriptor| descriptor.tools.strip_contains(tool))
}
