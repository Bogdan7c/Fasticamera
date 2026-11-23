//! Смена режима: что обязано произойти при переходе PHOTO ↔ CALIBRATE (решения 185, 203).
//!
//! Режим сам по себе — только запись в реестре (`ModeBehaviour`); здесь собраны побочные следствия смены:
//! панели закрываются, отсчёт таймера и «чистяк» не переживают режим, видоискатель переезжает на новое место.
//! Прибор показывает или скрывает общая сверка `calibration::sync_after_action`, а не этот модуль.

use super::{Ctx, calibration, menu, shutter};
use crate::effect::Effect;
use crate::state::{CardOrigin, PanelState};

/// Выполняет следствия уже выставленного нового режима.
pub(super) fn apply_mode_change(ctx: &mut Ctx<'_>) {
    // Меню Controls и карточки принадлежат режиму: после смены они не должны висеть в чужом режиме.
    ctx.state.panel = PanelState::Closed;
    ctx.state.card_origin = CardOrigin::default();
    // То же для меню калибровки и взведённого удаления.
    ctx.state
        .calibration
        .set_menu(crate::calibration::CalibrationMenu::Closed);
    ctx.state
        .calibration
        .set_delete_confirmation(crate::calibration::DeleteConfirmation::Idle);
    // «Чистяк» не залипает: иначе превью осталось бы «без вида» в режиме, где кнопки нет.
    calibration::release_clean_view(ctx);
    // Отсчёт таймера без затвора бессмыслен: снимок после выхода из режима стал бы сюрпризом.
    if !ctx.state.mode_behaviour().has_shutter() {
        shutter::cancel_countdown(ctx.state);
    }
    // Квадрат фокуса принадлежит старому кадру: на новом месте видоискателя он стоял бы не там.
    menu::hide_focus_indicator_and_release_camera_point(ctx);
    if let Some(intent) = ctx.state.framing_intent() {
        ctx.emit(Effect::Viewfinder(intent));
    }
}
