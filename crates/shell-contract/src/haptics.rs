//! Хаптика в идиоме iOS (`UIFeedbackGenerator`).
//!
//! Фича говорит, *что* почувствовать ([`HapticPattern`]), а не *как* вибрировать.
//! Как именно воспроизвести паттерн на конкретном вибромоторе (примитив, готовый эффект или
//! своя амплитудная волна) решает реализация сервиса, а не фича (решение владельца, сессия 02).

use serde::{Deserialize, Serialize};

/// Паттерн отклика по образцу генераторов iOS.
// `Deserialize` — чтобы таблицы «событие → паттерн» (`ui-kit`, сессия 10) задавались в конфиге
// этим же словарём, без копии списка паттернов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HapticPattern {
    /// `UISelectionFeedbackGenerator`: щелчок колеса (зум, переключатель режимов).
    SelectionTick,
    /// `UIImpactFeedbackGenerator(.light)`.
    LightImpact,
    /// `UIImpactFeedbackGenerator(.medium)`: например, затвор.
    MediumImpact,
    /// `UIImpactFeedbackGenerator(.heavy)`.
    HeavyImpact,
    /// `UIImpactFeedbackGenerator(.soft)`: мягкий короткий толчок.
    SoftImpact,
    /// `UIImpactFeedbackGenerator(.rigid)`: жёсткий короткий щелчок.
    RigidImpact,
    /// `UINotificationFeedbackGenerator(.success)`.
    Success,
    /// `UINotificationFeedbackGenerator(.warning)`.
    Warning,
    /// `UINotificationFeedbackGenerator(.error)`.
    Error,
}

impl HapticPattern {
    /// Все паттерны — для smoke-сценария и полноты таблиц.
    pub const ALL: [HapticPattern; 9] = [
        HapticPattern::SelectionTick,
        HapticPattern::LightImpact,
        HapticPattern::MediumImpact,
        HapticPattern::HeavyImpact,
        HapticPattern::SoftImpact,
        HapticPattern::RigidImpact,
        HapticPattern::Success,
        HapticPattern::Warning,
        HapticPattern::Error,
    ];

    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            HapticPattern::SelectionTick => "selection_tick",
            HapticPattern::LightImpact => "light_impact",
            HapticPattern::MediumImpact => "medium_impact",
            HapticPattern::HeavyImpact => "heavy_impact",
            HapticPattern::SoftImpact => "soft_impact",
            HapticPattern::RigidImpact => "rigid_impact",
            HapticPattern::Success => "success",
            HapticPattern::Warning => "warning",
            HapticPattern::Error => "error",
        }
    }
}

/// Каким способом паттерн воспроизводится на этом телефоне.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HapticRealizationKind {
    /// Примитив композиции (`VibrationEffect.Composition`).
    Primitive,
    /// Готовый эффект системы (`VibrationEffect.createPredefined`).
    PredefinedEffect,
    /// Своя амплитудная волна (`VibrationEffect.createWaveform`).
    Waveform,
}

/// Итог команды «сыграть паттерн».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", content = "realization", rename_all = "snake_case")]
pub enum HapticOutcome {
    /// Команда поставлена в очередь платформы указанным способом.
    Queued(HapticRealizationKind),
    /// У телефона нет способа воспроизвести паттерн (нет вибромотора или ни один способ
    /// не поддержан). Это не молчание: фича узнаёт, что отклика не будет.
    Unsupported,
    /// Activity сейчас нет (приложение закрыто или ещё не создано).
    NotAttached,
    /// Очередь команд переполнена — паттерн отброшен и посчитан.
    Backpressure,
}

/// Сервис хаптики. Потребители (экран камеры) тестируются с fake-реализацией.
pub trait HapticsService: Send + Sync {
    /// Ставит паттерн на воспроизведение и сразу возвращается (не ждёт вибромотор).
    fn play(&self, pattern: HapticPattern) -> HapticOutcome;
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn all_patterns_are_unique_and_named_like_serde() {
        let names: HashSet<_> = HapticPattern::ALL.iter().map(|pattern| pattern.name()).collect();
        assert_eq!(names.len(), HapticPattern::ALL.len());
        for pattern in HapticPattern::ALL {
            assert_eq!(
                serde_json::to_value(pattern).expect("json"),
                serde_json::json!(pattern.name())
            );
        }
    }

    #[test]
    fn outcome_serializes_with_tag() {
        let json = serde_json::to_value(HapticOutcome::Queued(HapticRealizationKind::Waveform)).expect("json");
        assert_eq!(
            json,
            serde_json::json!({"outcome": "queued", "realization": "waveform"})
        );
        let json = serde_json::to_value(HapticOutcome::Unsupported).expect("json");
        assert_eq!(json, serde_json::json!({"outcome": "unsupported"}));
    }
}
