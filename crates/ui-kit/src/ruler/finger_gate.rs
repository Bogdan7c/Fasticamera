//! Затвор пальца: отделяет значения, которые ведёт палец, от эха программных перемещений шкалы.

use shell_contract::haptics::HapticPattern;

use crate::scale::ScaleEvent;

/// Событие шкалы после затвора: только то, что экрану нужно как намерение.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GatedEvent<V> {
    /// Значение сменилось, и его ведёт палец: экран превращает его в действие.
    FingerValue(V),
    /// Что сыграть пальцу.
    Haptic(HapticPattern),
}

/// Кто сейчас двигает шкалу: палец (события значения становятся намерениями) или программа (события гасятся).
///
/// Это единственное место с правилом «эхо не уходит в действия»: оно общее для Exposure и ручек вида.
#[derive(Debug, Default)]
pub struct FingerGate {
    finger_driven: bool,
    /// Сколько событий значения погашено как эхо программной установки (для диагностики и тестов).
    suppressed_echo_events: u64,
}

impl FingerGate {
    /// Палец взял шкалу (`Began` пана).
    pub fn finger_took_over(&mut self) {
        self.finger_driven = true;
    }

    /// Программа взяла шкалу: дальнейшие события значения — эхо.
    pub fn program_took_over(&mut self) {
        self.finger_driven = false;
    }

    /// Ведёт ли шкалу палец сейчас (до `Settled`).
    pub fn is_finger_driven(&self) -> bool {
        self.finger_driven
    }

    /// Сколько значений погашено как эхо.
    pub fn suppressed_echo_events(&self) -> u64 {
        self.suppressed_echo_events
    }

    /// События шкалы → намерения. Порядок сохраняется; `Settled` возвращает шкалу программе.
    pub fn route<V>(&mut self, events: Vec<ScaleEvent<V>>) -> Vec<GatedEvent<V>> {
        let mut routed = Vec::new();
        for event in events {
            match event {
                ScaleEvent::ValueChanged { value } if self.finger_driven => routed.push(GatedEvent::FingerValue(value)),
                ScaleEvent::ValueChanged { .. } => self.suppressed_echo_events += 1,
                ScaleEvent::Haptic { pattern } => routed.push(GatedEvent::Haptic(pattern)),
                ScaleEvent::Settled { .. } => self.finger_driven = false,
                ScaleEvent::TickCrossed { .. } | ScaleEvent::ReferenceCrossed { .. } => {}
            }
        }
        routed
    }
}
