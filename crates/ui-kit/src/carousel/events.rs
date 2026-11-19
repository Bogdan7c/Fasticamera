//! События карусели.

use shell_contract::haptics::HapticPattern;

use crate::event_queue::{CarriesHaptic, EventOrigin};

/// Чем вызван выбор элемента.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionCause {
    /// Свайп (бросок или отпускание после перетаскивания).
    Swipe,
    /// Тап по элементу.
    Tap,
    /// Программно (состояние изменилось само, список заменён). Вибрации нет.
    Programmatic,
}

impl SelectionCause {
    /// Чьё движение: тап и свайп — палец (вибрация есть), программный выбор — нет.
    pub(crate) fn origin(self) -> EventOrigin {
        match self {
            SelectionCause::Swipe | SelectionCause::Tap => EventOrigin::Finger,
            SelectionCause::Programmatic => EventOrigin::Program,
        }
    }
}

/// Событие карусели; приходят из [`super::SnapCarousel::drain_events`] в порядке возникновения.
#[derive(Debug, Clone, PartialEq)]
pub enum CarouselEvent<T> {
    /// Выбран другой элемент. Ровно одно событие на смену.
    SelectionChanged {
        item: T,
        index: usize,
        cause: SelectionCause,
    },
    /// Что сыграть пальцу. Следует сразу за `SelectionChanged` при выборе от пальца.
    Haptic { pattern: HapticPattern },
}

/// Итог тапа по элементу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapOutcome {
    /// Выбран другой элемент, карусель едет к нему.
    Selected,
    /// Тапнули по уже выбранному (экран решает, что это значит, например, открыть меню).
    AlreadySelected,
}

impl<T> CarriesHaptic for CarouselEvent<T> {
    fn haptic(pattern: HapticPattern) -> Self {
        CarouselEvent::Haptic { pattern }
    }
}
