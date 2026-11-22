//! Состав автоматики 3A, который собирает корень композиции и отдаёт движку.
//!
//! Движок не создаёт исполнителей сам: он получает их готовыми (вендорские по умолчанию,
//! в 08a — наш AE) вместе с писателем «почтового ящика» баланса белого. Так замена
//! исполнителя и подключение потока рендера не требуют правки движка.

use camera_contract::auto_exposure::{ExposureDirective, ExposureProposalChannel};
use camera_contract::three_a::{DirectiveSlot, MainsFrequency, WhiteBalanceDirective};

use std::sync::Arc;

use crate::config::ThreeAConfig;
use crate::flash_status::FlashStillObserver;
use crate::three_a_control::{ExposureController, FocusController};
use crate::three_a_own::RawStatsExposure;
use crate::three_a_vendor::{VendorExposure, VendorFocus};

/// Исполнители 3A и ящик указаний баланса белого.
pub struct ThreeAWiring {
    pub exposure: Box<dyn ExposureController>,
    pub focus: Box<dyn FocusController>,
    /// Сюда движок кладёт режим AWB; читает поток рендера (решение 64).
    pub white_balance: DirectiveSlot<WhiteBalanceDirective>,
    /// Сюда движок кладёт указание нашему AE (где мерить, блокировка, что стоит в запросе);
    /// читает поток рендера (08a). С вендорским AE ящик просто никто не читает.
    pub exposure_directives: DirectiveSlot<ExposureDirective>,
    /// Наблюдатель итогов снимков со вспышкой (14d); по умолчанию его нет, и итог идёт только событием.
    pub flash_observer: Option<Arc<dyn FlashStillObserver>>,
}

impl ThreeAWiring {
    /// Состав сессии 08: вендорский AF, **временный** вендорский AE, пустой ящик AWB.
    pub fn vendor(config: &ThreeAConfig) -> Self {
        Self {
            exposure: Box::new(VendorExposure),
            focus: Box::new(VendorFocus::new(config.focus_marker_grace_frames)),
            white_balance: DirectiveSlot::new(),
            exposure_directives: DirectiveSlot::new(),
            flash_observer: None,
        }
    }

    /// Состав 08a: **наш** AE по статистике RAW и вендорский AF. `proposals` — ящик, в который
    /// пишет рендер; `mains` — частота сети по умолчанию (конфиг Android, решение 67).
    pub fn own(config: &ThreeAConfig, proposals: ExposureProposalChannel, mains: MainsFrequency) -> Self {
        Self::vendor(config).with_controllers(
            Box::new(RawStatsExposure::new(config.exposure, proposals, mains)),
            Box::new(VendorFocus::new(config.focus_marker_grace_frames)),
        )
    }

    /// Тот же состав, но с ящиком указаний AE, который уже держит читатель (поток рендера).
    pub fn with_exposure_directive_slot(mut self, slot: DirectiveSlot<ExposureDirective>) -> Self {
        self.exposure_directives = slot;
        self
    }

    /// Тот же состав, но с наблюдателем итогов снимков со вспышкой (14d).
    pub fn with_flash_observer(mut self, observer: Arc<dyn FlashStillObserver>) -> Self {
        self.flash_observer = Some(observer);
        self
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
