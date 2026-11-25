//! Панель белого: намерения движку и учёт отказов. Белое экран не хранит и не решает за движок: камеры нет,
//! поток не идёт — намерение всё равно уходит (движок ответит «не спрашивал»), экран его решение не дублирует.

use camera_contract::control::{ColorTemperature, Tint};
use camera_contract::white::{WhiteAdjustment, WhiteSourceId};
use shell_contract::haptics::HapticPattern;

use super::Ctx;
use super::menu::open_card_from_strip;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::CameraFacts;
use crate::outcome::{ActionOutcome, Rejection};
use crate::state::PanelState;
use crate::tools::ToolId;
use crate::white::{TakeVerdict, WhiteAxis};
use crate::white_config::WhiteLightIndex;

/// Открыта ли карточка белого.
fn card_is_open(ctx: &Ctx<'_>) -> bool {
    ctx.state.panel == PanelState::Submenu(ToolId::White)
}

/// Действия карточки при закрытой карточке — отказ, как у соседних подменю.
fn require_open_card(ctx: &Ctx<'_>) -> Result<(), ActionOutcome> {
    if card_is_open(ctx) {
        Ok(())
    } else {
        Err(ActionOutcome::Rejected(Rejection::OptionNotInSubmenu))
    }
}

/// Тап по кнопке белого рядом с затвором: открывает карточку белого, закрывает её, если она уже открыта, и
/// переключает на неё с другой карточки (правила значков верхнего ряда, `open_card_from_strip`). Пока открыто меню
/// Controls — отказ: шторка лежит над кнопкой.
pub(super) fn on_button_tapped(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if ctx.state.panel == PanelState::ControlsMenu {
        return ActionOutcome::Rejected(Rejection::PanelBlocksAction);
    }
    let was_open = card_is_open(ctx);
    open_card_from_strip(ctx, PanelState::Submenu(ToolId::White));
    if !was_open {
        // Карточка открывается заново: всегда с шкалы Кельвинов.
        ctx.state.white.reset_axis();
    }
    ActionOutcome::Applied
}

/// Долгое нажатие на кнопку белого: берёт белое телефона и подтверждает вибрацией (как блокировка AE/AF).
/// Панель не открывается и не закрывается: ни своя, ни чужая карточка не трогаются.
pub(super) fn on_button_long_pressed(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if ctx.state.panel == PanelState::ControlsMenu {
        return ActionOutcome::Rejected(Rejection::PanelBlocksAction);
    }
    ctx.emit(Effect::Camera(ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE)));
    ctx.emit(Effect::Haptic(HapticPattern::MediumImpact));
    ActionOutcome::Applied
}

/// Тап «Телефон» на карточке: то же намерение, вибрации нет (тап по кнопке карточки, решение 112).
pub(super) fn on_phone_tapped(ctx: &mut Ctx<'_>) -> ActionOutcome {
    if let Err(rejected) = require_open_card(ctx) {
        return rejected;
    }
    ctx.emit(Effect::Camera(ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE)));
    ActionOutcome::Applied
}

/// Кнопка света: белое целиком по числам кнопки.
pub(super) fn on_light_selected(ctx: &mut Ctx<'_>, index: WhiteLightIndex) -> ActionOutcome {
    if let Err(rejected) = require_open_card(ctx) {
        return rejected;
    }
    let setting = ctx
        .config
        .white
        .lights
        .get(index.0)
        .and_then(|light| light.setting().ok());
    let Some(setting) = setting else {
        return ActionOutcome::Rejected(Rejection::UnknownWhiteLight);
    };
    ctx.emit(Effect::Camera(ScreenCameraIntent::SetWhite(setting)));
    ActionOutcome::Applied
}

/// Переключатель шкалы карточки. Та же шкала — `NoChange` (общая проверка редьюсера: состояние не изменилось).
pub(super) fn on_axis_selected(ctx: &mut Ctx<'_>, axis: WhiteAxis) -> ActionOutcome {
    if let Err(rejected) = require_open_card(ctx) {
        return rejected;
    }
    ctx.state.white.select_axis(axis);
    ActionOutcome::Applied
}

/// Шкала Кельвинов под пальцем: точечная правка температуры.
pub(super) fn on_temperature_dragged(ctx: &mut Ctx<'_>, temperature: ColorTemperature) -> ActionOutcome {
    adjust(ctx, WhiteAdjustment::Temperature(temperature))
}

/// Шкала оттенка под пальцем: точечная правка оттенка.
pub(super) fn on_tint_dragged(ctx: &mut Ctx<'_>, tint: Tint) -> ActionOutcome {
    adjust(ctx, WhiteAdjustment::Tint(tint))
}

/// Намерение отправляется всегда, без сравнения с фактами: факты отстают на кадр, а «вернуть палец к значению,
/// которое ещё показывают факты» — настоящая правка.
fn adjust(ctx: &mut Ctx<'_>, adjustment: WhiteAdjustment) -> ActionOutcome {
    if let Err(rejected) = require_open_card(ctx) {
        return rejected;
    }
    ctx.emit(Effect::Camera(ScreenCameraIntent::AdjustWhite(adjustment)));
    ActionOutcome::Applied
}

/// Исполнитель не доставил команду «взять»: тряска и вибрация, как при отказе движка.
pub(super) fn on_take_not_delivered(ctx: &mut Ctx<'_>) -> ActionOutcome {
    ctx.state.white.register_undelivered_take();
    ctx.emit(Effect::Haptic(HapticPattern::Error));
    ActionOutcome::Applied
}

/// Факты камеры: новый отказ команды «взять» трясёт кнопку и вибрирует ровно один раз на номер команды.
pub(super) fn on_facts(ctx: &mut Ctx<'_>, facts: &CameraFacts) {
    if ctx.state.white.observe_take(facts.white.take) == TakeVerdict::NewRefusal {
        ctx.emit(Effect::Haptic(HapticPattern::Error));
    }
}
