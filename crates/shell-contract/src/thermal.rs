//! Показания тепла: статус троттлинга системы и «запас до троттлинга».
//!
//! Это сырьё для [`crate::thermal_policy::ThermalPolicy`]. Фичи читают не датчики, а решение
//! политики (задел V8 README: тепловые решения — в одном владельце).

use std::time::Duration;

use serde::Serialize;

/// Статус троттлинга по шкале Android (`PowerManager.THERMAL_STATUS_*`, `AThermalStatus`).
/// Порядок вариантов — по возрастанию тяжести, поэтому `Ord` сравнивает тяжесть.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalStatus {
    None,
    Light,
    Moderate,
    Severe,
    Critical,
    Emergency,
    Shutdown,
}

impl ThermalStatus {
    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            ThermalStatus::None => "none",
            ThermalStatus::Light => "light",
            ThermalStatus::Moderate => "moderate",
            ThermalStatus::Severe => "severe",
            ThermalStatus::Critical => "critical",
            ThermalStatus::Emergency => "emergency",
            ThermalStatus::Shutdown => "shutdown",
        }
    }
}

/// Запас до троттлинга: 0 — холодно, 1.0 — порог `Severe`, больше 1.0 — уже тяжёлый троттлинг.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalHeadroom {
    /// Измеренное значение (конечное и неотрицательное).
    Measured(f32),
    /// Система не дала значения: не поддерживается или запрос слишком частый (NaN у Android).
    Unavailable,
}

impl ThermalHeadroom {
    /// Из сырого значения платформы: NaN, бесконечность и отрицательные — `Unavailable`.
    pub fn from_platform_value(value: f32) -> Self {
        if value.is_finite() && value >= 0.0 {
            ThermalHeadroom::Measured(value)
        } else {
            ThermalHeadroom::Unavailable
        }
    }

    /// Значение, если оно измерено.
    pub fn measured(self) -> Option<f32> {
        match self {
            ThermalHeadroom::Measured(value) => Some(value),
            ThermalHeadroom::Unavailable => None,
        }
    }
}

/// Горизонт прогноза запаса. `NOW` — текущее значение без прогноза.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadroomForecast(Duration);

impl HeadroomForecast {
    pub const NOW: HeadroomForecast = HeadroomForecast(Duration::ZERO);

    pub const fn ahead(horizon: Duration) -> Self {
        Self(horizon)
    }

    pub const fn horizon(self) -> Duration {
        self.0
    }
}

/// Одно показание тепла.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ThermalReading {
    pub status: ThermalStatus,
    pub headroom: ThermalHeadroom,
}

/// Почему источник не дал статус.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThermalSourceError {
    #[error("тепловой сервис недоступен: {0}")]
    Unavailable(String),
}

/// Источник тепловых показаний (на телефоне — NDK `AThermal_*`, в тестах — fake).
pub trait ThermalSource: Send + Sync {
    /// Текущий статус троттлинга.
    fn current_status(&self) -> Result<ThermalStatus, ThermalSourceError>;

    /// Запас до троттлинга. Android разрешает спрашивать не чаще раза в секунду,
    /// иначе отвечает «нет значения» — это `Unavailable`, а не ошибка.
    fn headroom(&self, forecast: HeadroomForecast) -> ThermalHeadroom;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_platform_headroom_is_unavailable() {
        assert_eq!(
            ThermalHeadroom::from_platform_value(f32::NAN),
            ThermalHeadroom::Unavailable
        );
        assert_eq!(
            ThermalHeadroom::from_platform_value(f32::INFINITY),
            ThermalHeadroom::Unavailable
        );
        assert_eq!(ThermalHeadroom::from_platform_value(-0.1), ThermalHeadroom::Unavailable);
        assert_eq!(ThermalHeadroom::from_platform_value(0.46).measured(), Some(0.46));
    }

    #[test]
    fn status_order_is_severity() {
        assert!(ThermalStatus::None < ThermalStatus::Light);
        assert!(ThermalStatus::Severe < ThermalStatus::Shutdown);
    }

    #[test]
    fn status_name_matches_serde() {
        assert_eq!(
            serde_json::to_value(ThermalStatus::Moderate).expect("json"),
            serde_json::json!(ThermalStatus::Moderate.name())
        );
    }
}
