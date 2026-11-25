//! Охрана по режиму: действия управления камерой допустимы только там, где `ModeBehaviour` говорит, что управление
//! есть (решения 94, 185).
//!
//! Раньше эту границу держала только сцена (`Stage` не рисует фокус и зум в CALIBRATE). Редьюсер обязан держать её сам:
//! действие может прийти не от виджета (повтор очереди, другой экран, тест), и фокус на кадре калибровки или зум
//! под стеклом ручек были бы нарушением инварианта «из раздела не снимают, камерой не управляют».

use super::Ctx;
use crate::action::ScreenAction;
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
            | ScreenAction::WhiteButtonTapped
            | ScreenAction::WhiteButtonLongPressed
            | ScreenAction::WhiteAutoTapped
            | ScreenAction::WhitePhoneTapped
            | ScreenAction::WhiteLightSelected(_)
            | ScreenAction::WhiteAxisSelected(_)
            | ScreenAction::WhiteTemperatureDragged(_)
            | ScreenAction::WhiteTintDragged(_)
            | ScreenAction::ZoomStopTapped { .. }
            | ScreenAction::ZoomStripPressBegan { .. }
            | ScreenAction::ZoomWheelTouched { .. }
            | ScreenAction::ZoomWheelDragged { .. }
            | ScreenAction::ZoomPinchBegan { .. }
            | ScreenAction::ZoomPinchChanged { .. }
    )
}

/// `Err` — готовый отказ: действие управления камерой в режиме без такого управления.
pub(super) fn require_camera_controls(ctx: &Ctx<'_>, action: &ScreenAction) -> Result<(), ActionOutcome> {
    if is_camera_control(action) && !ctx.state.mode_behaviour().has_camera_controls() {
        return Err(ActionOutcome::Rejected(Rejection::NotAvailableInMode));
    }
    Ok(())
}
