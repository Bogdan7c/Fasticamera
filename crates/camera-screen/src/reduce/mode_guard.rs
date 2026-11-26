//! Охрана по режиму: действия управления камерой допустимы только там, где `ModeBehaviour` говорит, что управление
//! есть (решения 94, 185).
//!
//! Раньше эту границу держала только сцена (`Stage` не рисует фокус и зум в CALIBRATE). Редьюсер обязан держать её сам:
//! действие может прийти не от виджета (повтор очереди, другой экран, тест), и фокус на кадре калибровки или зум
//! под стеклом ручек были бы нарушением инварианта «из раздела не снимают, камерой не управляют».

use super::Ctx;
use crate::action::ScreenAction;
use crate::mode_behaviour::WhiteControls;
use crate::outcome::{ActionOutcome, Rejection};

/// Относится ли действие к управлению камерой (фокус, экспозиция, зум, значки над кадром).
///
/// Завершающие действия жестов (отпускание ленты, колеса, конец щипка) НЕ входят: они только заканчивают уже начатое
/// и должны проходить всегда, иначе жест, начатый в PHOTO, не смог бы закрыться после смены режима.
const fn is_camera_control(action: &ScreenAction) -> bool {
    matches!(
        action,
        ScreenAction::ViewfinderTapped { .. }
            | ScreenAction::ViewfinderLongPressed { .. }
            | ScreenAction::ExposureDragged { .. }
            | ScreenAction::FocusSunHeld { .. }
            | ScreenAction::ExposureResetRequested
            | ScreenAction::StripToolTapped(_)
            | ScreenAction::ZoomStopTapped { .. }
            | ScreenAction::ZoomStripPressBegan { .. }
            | ScreenAction::ZoomWheelTouched { .. }
            | ScreenAction::ZoomWheelDragged { .. }
            | ScreenAction::ZoomPinchBegan { .. }
            | ScreenAction::ZoomPinchChanged { .. }
    )
}

/// Относится ли действие к «полному» белому, которого у раздела калибровки нет: «Auto» и долгое нажатие на кнопку у
/// затвора (решение 294: в разделе только ручное белое и «Phone», слежение включают в PHOTO).
///
/// Остальное белое (тап по кнопке, выбор оси, шкалы, «Phone», кнопки света) режиму калибровки ДОСТУПНО: у раздела
/// своя капсула в полоске прибора и своя карточка (решения 290, 292), а допустимость каждого действия внутри
/// решает редьюсер белого — «карточка открыта» (`ScreenState::is_white_card_open`).
const fn is_full_white_control(action: &ScreenAction) -> bool {
    matches!(
        action,
        ScreenAction::WhiteButtonLongPressed | ScreenAction::WhiteAutoTapped
    )
}

/// `Err` — готовый отказ: действие управления камерой в режиме без такого управления.
pub(super) fn require_camera_controls(ctx: &Ctx<'_>, action: &ScreenAction) -> Result<(), ActionOutcome> {
    let behaviour = ctx.state.mode_behaviour();
    let rejected_as_camera_control = is_camera_control(action) && !behaviour.has_camera_controls();
    let rejected_as_full_white = is_full_white_control(action) && behaviour.white_controls != WhiteControls::Full;
    if rejected_as_camera_control || rejected_as_full_white {
        return Err(ActionOutcome::Rejected(Rejection::NotAvailableInMode));
    }
    Ok(())
}
