//! Настройки сценария `standby` (13b): сроки ожиданий и пороги. Секция `scenarios.standby` конфига нативной части.
//!
//! Это данные, а не код (правило «не хардкодить»): цифры `[estimate]` уточняются по замерам телефона.

use serde::{Deserialize, Serialize};

/// Настройки сценария `standby`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StandbyScenarioConfig {
    /// Сколько ждать, пока камера встанет «домой»: поток идёт, задняя основная на 1×, тёплых нет.
    /// `[estimate]` 15 с: с запасом на автозапуск и закрытие тёплых прошлого прогона.
    pub home_timeout_ms: u64,
    /// Окно замера частоты одной камеры (до просьбы о тёплой), мс. `[estimate]` 1500.
    pub alone_window_ms: u64,
    /// Сколько ждать готовности тёплой после просьбы, мс. Замер зонда — ≈ 200 мс; `[estimate]` 5 с.
    pub warm_timeout_ms: u64,
    /// Окно замера частоты, пока стримят обе камеры, мс. Перед каждым обменом и после него.
    /// `[estimate]` 2000: меньше — частота на нескольких десятках кадров слишком шумна.
    pub both_window_ms: u64,
    /// Сколько ждать смены объектива обменом ролей после просьбы, мс. `[estimate]` 4 с.
    pub swap_timeout_ms: u64,
    /// Сколько ждать, пока после «не нужно» все тёплые закроются, мс. Обязано быть больше паузы движка
    /// `camera.engine.standby.release_delay_ms` (1500 по умолчанию). `[estimate]` 8 с.
    pub release_timeout_ms: u64,
    /// Во сколько раз коэффициент плавного жеста при подъёме выше собственного коэффициента объектива:
    /// гистерезис селектора не пускает переключение вверх раньше `native × (1 + switch_up_hysteresis)`
    /// (0,05 по умолчанию). `[derived]` 1,10 — вдвое больше гистерезиса.
    pub overshoot_ratio: f32,
    /// Наименьшая допустимая частота каждой задней камеры, пока стримят обе, к/с (решение владельца: 55 из 60).
    pub min_back_fps: f64,
}

impl Default for StandbyScenarioConfig {
    fn default() -> Self {
        Self {
            home_timeout_ms: 15_000,
            alone_window_ms: 1_500,
            warm_timeout_ms: 5_000,
            both_window_ms: 2_000,
            swap_timeout_ms: 4_000,
            release_timeout_ms: 8_000,
            overshoot_ratio: 1.10,
            min_back_fps: 55.0,
        }
    }
}

/// Неверные настройки сценария `standby`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StandbyScenarioConfigError {
    #[error("scenarios.standby.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("scenarios.standby.overshoot_ratio должно быть конечным и больше 1, а не {0}")]
    InvalidOvershoot(f32),
    #[error("scenarios.standby.min_back_fps должно быть конечным и больше нуля, а не {0}")]
    InvalidMinFps(f64),
}

impl StandbyScenarioConfig {
    pub fn validate(&self) -> Result<(), StandbyScenarioConfigError> {
        for (field, value) in [
            ("home_timeout_ms", self.home_timeout_ms),
            ("alone_window_ms", self.alone_window_ms),
            ("warm_timeout_ms", self.warm_timeout_ms),
            ("both_window_ms", self.both_window_ms),
            ("swap_timeout_ms", self.swap_timeout_ms),
            ("release_timeout_ms", self.release_timeout_ms),
        ] {
            if value == 0 {
                return Err(StandbyScenarioConfigError::Zero { field });
            }
        }
        if !(self.overshoot_ratio.is_finite() && self.overshoot_ratio > 1.0) {
            return Err(StandbyScenarioConfigError::InvalidOvershoot(self.overshoot_ratio));
        }
        if !(self.min_back_fps.is_finite() && self.min_back_fps > 0.0) {
            return Err(StandbyScenarioConfigError::InvalidMinFps(self.min_back_fps));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "standby_config_tests.rs"]
mod tests;
