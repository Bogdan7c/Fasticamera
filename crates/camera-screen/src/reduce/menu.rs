//! Меню Controls, подменю, значки над кадром, карусель режимов и настройки съёмки.

use super::Ctx;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::modes::CaptureMode;
use crate::outcome::{ActionOutcome, NoChangeReason, Rejection};
use crate::settings::AspectRatio;
use crate::state::{CardOrigin, PanelState};
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

/// Закрыть панель свайпом вниз: одно правило для меню Controls, подменю и любой карточки (в том числе
/// открытой значком над кадром). `[estimate]`: для карточки со значка жест не снят, берём то же закрытие,
/// что у остальных панелей, чтобы жест не зависел от происхождения. Закрывать нечего — `NoChange`.
pub(super) fn on_close(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if ctx.state.panel == PanelState::Closed {
        return ActionOutcome::NoChange(NoChangeReason::NothingToClose);
    }
    ctx.state.panel = PanelState::Closed;
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
    ctx.state.card_origin = CardOrigin::ControlsMenu;
    ActionOutcome::Applied
}

/// Панель, в которую ведёт левая кнопка карточки: в меню Controls, если карточка открыта из него,
/// и в никуда, если значком над кадром.
fn panel_after_card_left_button(origin: CardOrigin) -> PanelState {
    match origin {
        CardOrigin::ControlsMenu => PanelState::ControlsMenu,
        CardOrigin::StripIcon => PanelState::Closed,
    }
}

/// Вариант в подменю: применяется настройка, карточка ОСТАЁТСЯ открытой (решение 103).
pub(super) fn on_option_selected(ctx: &mut Ctx<'_>, option: ToolOption) -> ActionOutcome {
    if ctx.state.panel != PanelState::Submenu(option.tool()) {
        return ActionOutcome::Rejected(Rejection::OptionNotInSubmenu);
    }
    if !apply_option(ctx, option) {
        return ActionOutcome::Rejected(Rejection::UnsupportedOption);
    }
    ActionOutcome::Applied
}

/// Левая кнопка карточки («‹» из меню, «×» со значка) — для подменю и для панели Exposure.
pub(super) fn on_submenu_back(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if ctx.state.is_tool_card_open() {
        ctx.state.panel = panel_after_card_left_button(ctx.state.card_origin);
    }
    ActionOutcome::Applied
}

/// Тап по значку над кадром.
///
/// Пока открыто МЕНЮ — отказ (значки под шторкой). Пока открыта КАРТОЧКА — тап переключает на
/// карточку/переключатель этого инструмента: верхний ряд в iOS 27 виден над карточкой (`[estimate]`:
/// не снято). Повторный тап по значку открытой карточки (Exposure, Timer) её закрывает.
pub(super) fn on_strip_tool_tapped(ctx: &mut Ctx<'_>, tool: ToolId) -> ActionOutcome {
    if ctx.state.panel == PanelState::ControlsMenu {
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
            // Цикл iOS 26: Off → Auto → On → Off.
            let next = ctx.state.settings.flash.next_in_cycle();
            ctx.state.settings.flash = next;
        }
        ToolId::Raw => ctx.state.settings.raw = ctx.state.settings.raw.toggled(),
        ToolId::Timer => {
            open_card_from_strip(ctx, PanelState::Submenu(ToolId::Timer));
        }
        ToolId::Exposure => {
            open_card_from_strip(ctx, PanelState::ExposurePanel);
        }
        // ToolSet::new не пускает в ряд значков другие инструменты; ветка недостижима, но отказ явный.
        ToolId::Aspect | ToolId::Grid | ToolId::Level | ToolId::Live | ToolId::Styles | ToolId::Night => {
            return ActionOutcome::Rejected(Rejection::ToolNotInStrip);
        }
    }
    ActionOutcome::Applied
}

/// Открывает карточку тапом по значку; если она уже открыта — закрывает (переключатель).
fn open_card_from_strip(ctx: &mut Ctx<'_>, card: PanelState) {
    if ctx.state.panel == card {
        ctx.state.panel = PanelState::Closed;
    } else {
        ctx.state.panel = card;
        ctx.state.card_origin = CardOrigin::StripIcon;
    }
}

/// Применяет вариант к настройкам. `false` — вариант не поддержан (например, вспышки у камеры нет).
fn apply_option(ctx: &mut Ctx<'_>, option: ToolOption) -> bool {
    match option {
        ToolOption::Flash(mode) => {
            // Вспышки у камеры нет (инвариант 6) — выбирать нечего.
            if !ctx.state.is_tool_available(ToolId::Flash) {
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
