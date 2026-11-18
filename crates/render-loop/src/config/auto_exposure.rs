//! Секция `viewfinder.auto_exposure`: настройки нашего AE на потоке рендера (сессия 08a).
//!
//! Числа замера и контроллера цели живут в `scene-estimation` (`MeteringConfig`,
//! `ExposureControllerConfig`); здесь — то, что решает поток рендера: как долго просить статистику
//! на каждом кадре, пока первой оценки сцены нет.

use scene_estimation::exposure::{ExposureControllerConfig, MeteringConfig};
use serde::{Deserialize, Serialize};

/// Настройки AE потока рендера.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AutoExposureConfig {
    /// Замер: веса плиток, пороги пересвета и темноты, шаги ограничения.
    pub metering: MeteringConfig,
    /// Контроллер цели: сглаживание, профили, блокировка, частота предложений, предел подтяжки.
    pub controller: ExposureControllerConfig,
    /// Сколько кадров подряд AE просит статистику на КАЖДОМ кадре, пока у него нет первой оценки сцены
    /// (новый поток, новая область тапа). Предел обязателен: если оценки не будет вовсе (у кадров нет
    /// выдержки и ISO, область вне сетки), стадия статистики не должна считаться на каждом кадре вечно.
    pub cold_start_every_frame_limit: u32,
    /// Шаг квантования остаточной подтяжки, EV. Подтяжка входит в ключ кэша параметров ISP: без квантования она
    /// дрожала бы в четвёртом знаке с каждым замером и параметры пересчитывались бы на каждом кадре. Со шагом
    /// кэш бьёт, пока `k` в пределах шага, а подтяжка меньше половины шага равна ровно 1 (старый путь бит-в-бит).
    pub residual_gain_step_ev: f32,
}

impl Default for AutoExposureConfig {
    /// `cold_start_every_frame_limit` 30 ≈ 0,5 с при 60 к/с. Статистика читается после fence слота
    /// (оценка готова на 3-м кадре), а потом сенсор догоняет экспозицию 7–8 кадров: полсекунды —
    /// запас в несколько раз на медленный GPU и пропущенные такты, дальше AE живёт по расписанию AWB.
    /// `residual_gain_step_ev` 1/32 EV ≈ 0,03: на порядок ниже порога, который видит глаз (~0,1 EV), и не больше
    /// 16 разных значений на весь диапазон ±0,5 EV. Оценка до замера на телефоне (08b/08c).
    fn default() -> Self {
        Self {
            metering: MeteringConfig::default(),
            controller: ExposureControllerConfig::default(),
            cold_start_every_frame_limit: 30,
            residual_gain_step_ev: 1.0 / 32.0,
        }
    }
}

/// Что не так с секцией.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutoExposureConfigError {
    /// Настройки замера отвергнуты.
    #[error("viewfinder.auto_exposure.metering: {0}")]
    Metering(String),
    /// Настройки контроллера отвергнуты.
    #[error("viewfinder.auto_exposure.controller: {0}")]
    Controller(String),
    /// Шаг квантования подтяжки не положителен или не конечен.
    #[error("viewfinder.auto_exposure.residual_gain_step_ev должен быть конечным и > 0, а не {0}")]
    ResidualGainStep(String),
}

impl AutoExposureConfig {
    /// Проверяет секцию.
    pub fn validate(&self) -> Result<(), AutoExposureConfigError> {
        self.metering
            .validate()
            .map_err(|error| AutoExposureConfigError::Metering(error.to_string()))?;
        self.controller
            .validate()
            .map_err(|error| AutoExposureConfigError::Controller(error.to_string()))?;
        if !(self.residual_gain_step_ev.is_finite() && self.residual_gain_step_ev > 0.0) {
            return Err(AutoExposureConfigError::ResidualGainStep(
                self.residual_gain_step_ev.to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let config = AutoExposureConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.cold_start_every_frame_limit, 30);
    }

    #[test]
    fn broken_nested_sections_and_unknown_fields_are_rejected() {
        let mut broken_metering = AutoExposureConfig::default();
        broken_metering.metering.dark_floor = -1.0;
        assert!(matches!(
            broken_metering.validate(),
            Err(AutoExposureConfigError::Metering(_))
        ));
        let mut broken_controller = AutoExposureConfig::default();
        broken_controller.controller.photo_time_constant_seconds = 0.0;
        assert!(matches!(
            broken_controller.validate(),
            Err(AutoExposureConfigError::Controller(_))
        ));
        let zero_step = AutoExposureConfig {
            residual_gain_step_ev: 0.0,
            ..AutoExposureConfig::default()
        };
        assert_eq!(
            zero_step.validate(),
            Err(AutoExposureConfigError::ResidualGainStep("0".to_owned()))
        );
        assert!(serde_json::from_str::<AutoExposureConfig>(r#"{"hidden":3}"#).is_err());
        let parsed: AutoExposureConfig =
            serde_json::from_str(r#"{"cold_start_every_frame_limit":5}"#).expect("секция читается");
        assert_eq!(parsed.cold_start_every_frame_limit, 5);
        assert_eq!(parsed.controller, ExposureControllerConfig::default());
    }
}
