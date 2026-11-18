//! Указание баланса белого для потока рендера (решение 64).
//!
//! Движок камеры решает, кто главный в AWB: автомат или пользователь. Решение кладётся в
//! [`DirectiveSlot`], который читает поток рендера. Публикуем при старте и **только при
//! изменении**: поколение слота растёт лишь на настоящих сменах, и читатель не перечитывает
//! то же самое.
//!
//! `Locked` (держать точку белого) в 08 недостижим: `ManualControl::white_balance` знает только
//! `Auto` и `Manual`. Режим нужен ручному режиму камеры (15). Блокировка AE/AF AWB **не**
//! блокирует — как в iOS.

use camera_contract::control::Control;
use camera_contract::three_a::{DirectiveSlot, WhiteBalanceDirective, WhiteBalanceMode};

use crate::three_a_control::ThreeAIntent;

/// Писатель ящика AWB, помнящий последнее опубликованное.
pub(super) struct WhiteBalancePublisher {
    slot: DirectiveSlot<WhiteBalanceDirective>,
    last_published: Option<WhiteBalanceDirective>,
}

impl WhiteBalancePublisher {
    pub(super) fn new(slot: DirectiveSlot<WhiteBalanceDirective>) -> Self {
        Self {
            slot,
            last_published: None,
        }
    }

    pub(super) fn publish_if_changed(&mut self, intent: &ThreeAIntent) {
        let directive = directive_for(intent);
        if self.last_published == Some(directive) {
            return;
        }
        self.slot.publish(directive);
        self.last_published = Some(directive);
    }
}

/// Чистая функция: намерение → указание AWB.
pub(super) fn directive_for(intent: &ThreeAIntent) -> WhiteBalanceDirective {
    let mode = match intent.controls.white_balance {
        Control::Auto => WhiteBalanceMode::Auto,
        Control::Manual(setting) => WhiteBalanceMode::Manual { setting },
    };
    WhiteBalanceDirective {
        mode,
        profile: intent.profile,
    }
}
