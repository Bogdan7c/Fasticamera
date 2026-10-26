//! Общие куски отчётов `render` и `render-lifecycle`: числа в миллисекундах, медиана, validation,
//! параметры swapchain. Чистые функции без состояния.

use std::collections::BTreeMap;
use std::time::Duration;

use render_loop::backend::DisplayInfo;
use render_loop::thread::RenderStatus;
use serde::Serialize;

use super::ScenarioOutcome;
use super::observing::judged;
use crate::gpu_services::ValidationState;

/// Вердикт проверки: отчёт и нарушения правил (пусто — прошёл).
#[derive(Debug)]
pub struct Verdict {
    pub report: serde_json::Value,
    pub violations: Vec<String>,
}

impl Verdict {
    /// Собирает вердикт; отчёт, который не сериализовался, сам становится нарушением.
    pub fn new(report: impl Serialize, mut violations: Vec<String>) -> Self {
        let report = serde_json::to_value(report).unwrap_or_else(|error| {
            violations.push(format!("отчёт не сериализован: {error}"));
            serde_json::Value::Null
        });
        Self { report, violations }
    }

    /// Итог сценария: `passed` с отчётом или `failed` с отчётом и перечнем нарушений.
    pub fn into_outcome(self) -> ScenarioOutcome {
        judged(self.report, self.violations)
    }
}

/// Секунды `Duration` как миллисекунды с дробью (в JSON-отчётах читать удобнее, чем наносекунды).
pub fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Медиана; для чётного числа значений — среднее двух средних. Пустой набор — `None`.
pub fn median(values: &[Duration]) -> Option<Duration> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[middle])
    } else {
        Some((sorted[middle - 1] + sorted[middle]) / 2)
    }
}

/// Validation в отчёте: режим и счётчики. В release режим `disabled`, и нули ничего не доказывают.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReportPart {
    pub mode: ValidationState,
    pub errors: u64,
    pub warnings: u64,
    /// Срабатывания известных отходов validation по VUID: справка, провалом не считается.
    pub known_deviations: BTreeMap<String, u64>,
}

impl ValidationReportPart {
    pub fn of(status: &RenderStatus, mode: ValidationState) -> Self {
        Self {
            mode,
            errors: status.backend.validation_errors,
            warnings: status.backend.validation_warnings,
            known_deviations: status.backend.known_deviations.clone(),
        }
    }

    /// Нарушение правила «0 ошибок validation»; в режиме `disabled` проверять нечего.
    pub fn violation(&self) -> Option<String> {
        (self.mode == ValidationState::Enabled && self.errors != 0)
            .then(|| format!("ошибки validation: {} (ожидалось 0)", self.errors))
    }
}

/// Параметры цепочки показа в отчёте.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DisplayReportPart {
    pub format: String,
    pub color_space: String,
    pub width_px: u32,
    pub height_px: u32,
    pub transform: String,
    pub image_count: usize,
    pub recreation_count: u64,
}

impl DisplayReportPart {
    pub fn of(info: &DisplayInfo) -> Self {
        Self {
            format: info.format.clone(),
            color_space: info.color_space.clone(),
            width_px: info.extent.width_px,
            height_px: info.extent.height_px,
            transform: info.transform.clone(),
            image_count: info.image_count,
            recreation_count: info.recreation_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn median_of_odd_even_and_empty() {
        assert_eq!(median(&[ms(30), ms(10), ms(20)]), Some(ms(20)));
        assert_eq!(median(&[ms(40), ms(10), ms(20), ms(30)]), Some(ms(25)));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn validation_violation_only_when_enabled() {
        let enabled = ValidationReportPart {
            mode: ValidationState::Enabled,
            errors: 2,
            warnings: 0,
            known_deviations: Default::default(),
        };
        assert!(enabled.violation().expect("нарушение").contains('2'));
        let disabled = ValidationReportPart {
            mode: ValidationState::Disabled,
            errors: 0,
            warnings: 0,
            known_deviations: Default::default(),
        };
        assert_eq!(disabled.violation(), None);
        let clean = ValidationReportPart {
            errors: 0,
            ..enabled.clone()
        };
        assert_eq!(clean.violation(), None);
    }
}
