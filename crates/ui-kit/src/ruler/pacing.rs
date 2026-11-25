//! Темп отдачи линейки: не давать щелчкам сыпаться каждый кадр при быстром взмахе.
//!
//! Шкала сама сжимает щелчки до одного на кадр, но на линейке с мелким шагом (ручки вида: риска через 20 pt)
//! быстрый взмах проходит риску в каждом кадре, и мотор превращается в гул. Для такой линейки включается
//! ограничитель частоты ([`HapticThrottle`], тот же, что у колеса зума): не чаще одного щелчка за интервал токена.
//! Щелчок на опорном значении («ноль») сильнее и важнее обычного: он ограничителем не глушится.

use shell_contract::haptics::HapticPattern;
use ui_core::time::MonotonicNanos;

use super::finger_gate::GatedEvent;
use crate::haptic_throttle::{HapticThrottle, ThrottleVerdict};
use crate::tokens::{HapticEvent, TokenError, UiKitTokens};

/// Как линейка ограничивает частоту щелчков.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HapticPacing {
    /// Каждый кадр, где пройдена риска, даёт щелчок (поведение по умолчанию: Exposure и остальные шкалы).
    #[default]
    Unlimited,
    /// Не чаще одного щелчка за интервал `UiKitTokens::haptic_throttle`; щелчок опорного значения проходит всегда.
    Throttled,
}

/// Исполнитель темпа: держит ограничитель и знает, какой паттерн у опорного значения.
#[derive(Debug, Clone)]
pub(super) struct HapticPacer {
    /// `None` — темп не ограничен.
    throttle: Option<HapticThrottle>,
    /// Паттерн щелчка опорного значения (его ограничитель пропускает всегда).
    reference_pattern: Option<HapticPattern>,
}

impl HapticPacer {
    pub(super) fn new(pacing: HapticPacing, tokens: &UiKitTokens) -> Result<Self, TokenError> {
        let throttle = match pacing {
            HapticPacing::Unlimited => None,
            HapticPacing::Throttled => Some(HapticThrottle::new(&tokens.haptic_throttle)?),
        };
        Ok(Self {
            throttle,
            reference_pattern: tokens.haptics.pattern_for(HapticEvent::ScaleReference),
        })
    }

    /// Сколько щелчков подавлено ограничителем (0, если темп не ограничен).
    pub(super) fn suppressed_count(&self) -> u64 {
        self.throttle.as_ref().map_or(0, HapticThrottle::suppressed_count)
    }

    /// Оставить из событий те щелчки, которые ограничитель пропускает в момент `now`; значения проходят как есть.
    pub(super) fn filter<V>(&mut self, events: Vec<GatedEvent<V>>, now: MonotonicNanos) -> Vec<GatedEvent<V>> {
        let Some(throttle) = self.throttle.as_mut() else {
            return events;
        };
        let reference_pattern = self.reference_pattern;
        events
            .into_iter()
            .filter(|event| match event {
                GatedEvent::Haptic(pattern) if Some(*pattern) == reference_pattern => true,
                GatedEvent::Haptic(_) => throttle.admit(now) == ThrottleVerdict::Play,
                GatedEvent::FingerValue(_) => true,
            })
            .collect()
    }
}
