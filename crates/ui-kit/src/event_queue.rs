//! Очередь событий элемента: «событие» и «хаптика, если причина не программная».
//!
//! Одна реализация для шкалы и карусели (правило «без дублирования»): элементы кладут события и
//! просят хаптику, а решение «играть ли её» принимается здесь по происхождению движения.

use shell_contract::haptics::HapticPattern;

/// Чьё движение породило событие: от него зависит, вибрируем ли.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventOrigin {
    /// Палец (касание, перетаскивание, тап, бросок после жеста): вибрация есть.
    Finger,
    /// Программная установка значения или выбора: вибрации нет.
    Program,
}

/// Событие элемента, умеющее нести паттерн хаптики.
pub(crate) trait CarriesHaptic {
    fn haptic(pattern: HapticPattern) -> Self;
}

/// Очередь событий элемента: порядок возникновения сохраняется, `drain` забирает всё.
#[derive(Debug, Clone)]
pub(crate) struct EventQueue<E> {
    events: Vec<E>,
}

impl<E: CarriesHaptic> EventQueue<E> {
    pub(crate) fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub(crate) fn push(&mut self, event: E) {
        self.events.push(event);
    }

    /// Поставить хаптику, если движение от пальца и таблица токенов задаёт паттерн для события
    /// (`None` — хаптика выключена токеном или события нет).
    pub(crate) fn push_haptic(&mut self, origin: EventOrigin, pattern: Option<HapticPattern>) {
        if origin == EventOrigin::Finger
            && let Some(pattern) = pattern
        {
            self.events.push(E::haptic(pattern));
        }
    }

    /// Забрать накопленные события (очередь очищается).
    pub(crate) fn drain(&mut self) -> Vec<E> {
        std::mem::take(&mut self.events)
    }
}
