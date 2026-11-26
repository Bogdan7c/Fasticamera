//! Лёгкий щелчок выбора из набора (решение 286): тап по вкладке колеса и по точке полосы.
//!
//! Один паттерн для всего раздела — событие `CarouselSelection` («выбор одного из набора», как смена режима в
//! карусели); нового токена не заводим: сила щелчка — в таблице токенов, а не в виджетах.

use ui_kit::tokens::HapticEvent;

use crate::widget::{Ctx, Outputs};

/// Играет щелчок выбора, если он включён токенами. Вызывающий сам решает, был ли выбор НОВЫМ: повторный тап по
/// уже выбранному щелчка не даёт (ничего не изменилось).
pub(super) fn play_selection_haptic(ctx: &Ctx<'_>, out: &mut Outputs) {
    if let Some(pattern) = ctx.tokens.haptics.pattern_for(HapticEvent::CarouselSelection) {
        out.haptics.push(pattern);
    }
}
