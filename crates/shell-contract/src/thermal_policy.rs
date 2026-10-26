//! Тепловая политика — единственный владелец тепловых решений (задел V8 README).
//!
//! Фичи (видоискатель, запись, ISP) не читают датчики тепла сами: они спрашивают у политики
//! [`ThermalPolicy::pressure`] и решают, как снизить нагрузку. Показания поставляет оболочка
//! (`ShellEvent::ThermalStatus`), маршрутизатор передаёт их в [`ThermalPolicy::observe`].
//! В сессии 02 политика только оценивает давление; ступени деградации появятся в 17.

use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::thermal::{ThermalReading, ThermalStatus};

/// Оценка теплового давления. Порядок — по возрастанию тяжести.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalPressure {
    /// Показаний ещё не было — честное «не знаю», а не «холодно».
    Unknown,
    Nominal,
    /// Нагрев заметен: тяжёлые необязательные задачи лучше не начинать.
    Elevated,
    /// Система уже троттлит умеренно.
    Throttling,
    /// Тяжёлый троттлинг или близко к нему: нагрузку снижать немедленно.
    Critical,
}

/// Пороги политики по запасу до троттлинга (1.0 = порог `Severe` у Android).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ThermalPolicyConfig {
    /// С какого запаса давление считается `Elevated`. По умолчанию 0.7: в покое у телефона
    /// 0,46 (сессия 00), значит 0,7 — это уже заметный нагрев.
    pub elevated_from_headroom: f32,
    /// С какого запаса давление `Critical`. По умолчанию 1.0 — порог `Severe` у Android.
    pub critical_from_headroom: f32,
}

impl Default for ThermalPolicyConfig {
    fn default() -> Self {
        Self {
            elevated_from_headroom: 0.7,
            critical_from_headroom: 1.0,
        }
    }
}

/// Ошибка конфигурации политики.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ThermalPolicyConfigError {
    #[error("пороги запаса должны быть конечными и 0 < elevated ({elevated}) < critical ({critical})")]
    InvalidThresholds { elevated: f32, critical: f32 },
}

impl ThermalPolicyConfig {
    fn validate(self) -> Result<Self, ThermalPolicyConfigError> {
        let elevated = self.elevated_from_headroom;
        let critical = self.critical_from_headroom;
        let valid = elevated.is_finite() && critical.is_finite() && elevated > 0.0 && elevated < critical;
        if valid {
            Ok(self)
        } else {
            Err(ThermalPolicyConfigError::InvalidThresholds { elevated, critical })
        }
    }
}

#[derive(Debug, Default)]
struct PolicyState {
    latest: Option<ThermalReading>,
    readings_observed: u64,
}

/// Тепловая политика процесса. Потокобезопасна: показания пишет маршрутизатор событий,
/// давление читают фичи с любых потоков.
#[derive(Debug)]
pub struct ThermalPolicy {
    config: ThermalPolicyConfig,
    state: Mutex<PolicyState>,
}

impl ThermalPolicy {
    pub fn new(config: ThermalPolicyConfig) -> Result<Self, ThermalPolicyConfigError> {
        Ok(Self {
            config: config.validate()?,
            state: Mutex::new(PolicyState::default()),
        })
    }

    /// Принимает новое показание (последнее побеждает).
    pub fn observe(&self, reading: ThermalReading) {
        let mut state = self.lock_state();
        state.latest = Some(reading);
        state.readings_observed += 1;
    }

    /// Текущее давление: худшее из оценки по статусу и оценки по запасу.
    pub fn pressure(&self) -> ThermalPressure {
        match self.lock_state().latest {
            None => ThermalPressure::Unknown,
            Some(reading) => self.assess(reading),
        }
    }

    /// Последнее показание, если оно было.
    pub fn latest_reading(&self) -> Option<ThermalReading> {
        self.lock_state().latest
    }

    /// Сколько показаний принято с начала процесса.
    pub fn readings_observed(&self) -> u64 {
        self.lock_state().readings_observed
    }

    fn assess(&self, reading: ThermalReading) -> ThermalPressure {
        let by_status = pressure_for_status(reading.status);
        let by_headroom = match reading.headroom.measured() {
            Some(headroom) if headroom >= self.config.critical_from_headroom => ThermalPressure::Critical,
            Some(headroom) if headroom >= self.config.elevated_from_headroom => ThermalPressure::Elevated,
            // Нет запаса — оценка только по статусу.
            Some(_) | None => ThermalPressure::Nominal,
        };
        by_status.max(by_headroom)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, PolicyState> {
        // Под замком только присваивания, паника внутри невозможна; если бы она случилась,
        // данные остаются согласованными (одно поле Option и счётчик), поэтому берём их как есть.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Статус Android → давление. Moderate — система уже троттлит; Severe и выше — критично.
fn pressure_for_status(status: ThermalStatus) -> ThermalPressure {
    match status {
        ThermalStatus::None => ThermalPressure::Nominal,
        ThermalStatus::Light => ThermalPressure::Elevated,
        ThermalStatus::Moderate => ThermalPressure::Throttling,
        ThermalStatus::Severe | ThermalStatus::Critical | ThermalStatus::Emergency | ThermalStatus::Shutdown => {
            ThermalPressure::Critical
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thermal::ThermalHeadroom;

    fn policy() -> ThermalPolicy {
        ThermalPolicy::new(ThermalPolicyConfig::default()).expect("политика")
    }

    fn reading(status: ThermalStatus, headroom: ThermalHeadroom) -> ThermalReading {
        ThermalReading { status, headroom }
    }

    #[test]
    fn without_readings_pressure_is_unknown() {
        let policy = policy();
        assert_eq!(policy.pressure(), ThermalPressure::Unknown);
        assert_eq!(policy.latest_reading(), None);
        assert_eq!(policy.readings_observed(), 0);
    }

    #[test]
    fn cold_phone_at_rest_is_nominal() {
        let policy = policy();
        // Значения покоя из сессии 00: статус 0, запас 0,46.
        policy.observe(reading(ThermalStatus::None, ThermalHeadroom::Measured(0.46)));
        assert_eq!(policy.pressure(), ThermalPressure::Nominal);
    }

    #[test]
    fn headroom_raises_pressure_before_status_does() {
        let policy = policy();
        policy.observe(reading(ThermalStatus::None, ThermalHeadroom::Measured(0.75)));
        assert_eq!(policy.pressure(), ThermalPressure::Elevated);
        policy.observe(reading(ThermalStatus::None, ThermalHeadroom::Measured(1.0)));
        assert_eq!(policy.pressure(), ThermalPressure::Critical);
    }

    #[test]
    fn status_wins_when_worse_than_headroom() {
        let policy = policy();
        policy.observe(reading(ThermalStatus::Moderate, ThermalHeadroom::Measured(0.1)));
        assert_eq!(policy.pressure(), ThermalPressure::Throttling);
        policy.observe(reading(ThermalStatus::Severe, ThermalHeadroom::Unavailable));
        assert_eq!(policy.pressure(), ThermalPressure::Critical);
    }

    #[test]
    fn latest_reading_wins_and_count_grows() {
        let policy = policy();
        policy.observe(reading(ThermalStatus::Severe, ThermalHeadroom::Unavailable));
        policy.observe(reading(ThermalStatus::None, ThermalHeadroom::Unavailable));
        assert_eq!(policy.pressure(), ThermalPressure::Nominal);
        assert_eq!(policy.readings_observed(), 2);
    }

    #[test]
    fn invalid_thresholds_are_rejected() {
        let inverted = ThermalPolicyConfig {
            elevated_from_headroom: 1.2,
            critical_from_headroom: 1.0,
        };
        assert!(matches!(
            ThermalPolicy::new(inverted),
            Err(ThermalPolicyConfigError::InvalidThresholds { .. })
        ));
        let nan = ThermalPolicyConfig {
            elevated_from_headroom: f32::NAN,
            critical_from_headroom: 1.0,
        };
        assert!(ThermalPolicy::new(nan).is_err());
    }

    #[test]
    fn config_rejects_unknown_fields_and_fills_defaults() {
        let partial: ThermalPolicyConfig =
            serde_json::from_str(r#"{"elevated_from_headroom": 0.6}"#).expect("частичный конфиг");
        assert_eq!(partial.elevated_from_headroom, 0.6);
        assert_eq!(partial.critical_from_headroom, 1.0);
        assert!(serde_json::from_str::<ThermalPolicyConfig>(r#"{"typo": 1}"#).is_err());
    }
}
