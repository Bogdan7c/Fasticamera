//! Состав автоматики 3A, который собирает корень композиции и отдаёт движку.
//!
//! Движок не создаёт исполнителей сам: он получает их готовыми (вендорские по умолчанию,
//! в 08a — наш AE) вместе с писателем «почтового ящика» баланса белого. Так замена
//! исполнителя и подключение потока рендера не требуют правки движка.

use camera_contract::three_a::{DirectiveSlot, WhiteBalanceDirective};

use crate::config::ThreeAConfig;
use crate::three_a_control::{ExposureController, FocusController};
use crate::three_a_vendor::{VendorExposure, VendorFocus};

/// Исполнители 3A и ящик указаний баланса белого.
pub struct ThreeAWiring {
    pub exposure: Box<dyn ExposureController>,
    pub focus: Box<dyn FocusController>,
    /// Сюда движок кладёт режим AWB; читает поток рендера (решение 64).
    pub white_balance: DirectiveSlot<WhiteBalanceDirective>,
}

impl ThreeAWiring {
    /// Состав сессии 08: вендорский AF, **временный** вендорский AE, пустой ящик AWB.
    pub fn vendor(config: &ThreeAConfig) -> Self {
        Self {
            exposure: Box::new(VendorExposure),
            focus: Box::new(VendorFocus::new(config.focus_marker_grace_frames)),
            white_balance: DirectiveSlot::new(),
        }
    }

    /// Тот же состав, но с ящиком, который уже держит читатель (поток рендера).
    pub fn with_white_balance_slot(mut self, slot: DirectiveSlot<WhiteBalanceDirective>) -> Self {
        self.white_balance = slot;
        self
    }

    /// Тот же состав, но с другими исполнителями (08a, тесты).
    pub fn with_controllers(mut self, exposure: Box<dyn ExposureController>, focus: Box<dyn FocusController>) -> Self {
        self.exposure = exposure;
        self.focus = focus;
        self
    }
}

impl std::fmt::Debug for ThreeAWiring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ThreeAWiring")
    }
}
