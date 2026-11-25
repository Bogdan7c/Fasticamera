//! Секция `viewfinder.auto_exposure`: настройки нашего AE на потоке рендера (сессия 08a).
//!
//! Числа замера и контроллера цели живут в `scene-estimation` (`MeteringConfig`,
//! `ExposureControllerConfig`); здесь — то, что решает поток рендера: как долго просить статистику
//! на каждом кадре, пока первой оценки сцены нет.

use scene_estimation::exposure::{ExposureControllerConfig, MeteringConfig, SceneChangeConfig};
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
    /// Установившийся период статистики кадра на GPU: она считается на каждом N-м обработанном кадре (1 — на
    /// каждом). Статистика нужна AE; пока кадры спрятаны холодным стартом белой или AE просит «каждый кадр»,
    /// считается чаще. Без AE на потоке рендера она не считается вовсе.
    pub statistics_every_n_frames: u32,
    /// Шаг квантования остаточной подтяжки, EV. Подтяжка входит в ключ кэша параметров ISP: без квантования она
    /// дрожала бы в четвёртом знаке с каждым замером и параметры пересчитывались бы на каждом кадре. Со шагом
    /// кэш бьёт, пока `k` в пределах шага, а подтяжка меньше половины шага равна ровно 1 (старый путь бит-в-бит).
    pub residual_gain_step_ev: f32,
    /// Детектор «сцена под точкой тапа сменилась» (решение 241): когда рендер сообщает движку, что точка устарела.
    pub scene_change: SceneChangeConfig,
}

impl Default for AutoExposureConfig {
    /// `cold_start_every_frame_limit` 30 ≈ 0,5 с при 60 к/с. Статистика читается после fence слота
    /// (оценка готова на 3-м кадре), а потом сенсор догоняет экспозицию 7–8 кадров: полсекунды —
    /// запас в несколько раз на медленный GPU и пропущенные такты, дальше AE живёт по расписанию статистики кадра.
    /// `statistics_every_n_frames` 4 → 15 Гц при 60 к/с. Стадия статистики стоила ≈ 55 мА из ≈ 119 мА всего ISP
    /// на телефоне (07a); свет сцены меняется медленнее (сглаживание контроллера ≥ 0,25 с), 15 Гц даёт не меньше
    /// трёх замеров за постоянную времени самого быстрого профиля.
    /// `residual_gain_step_ev` 1/32 EV ≈ 0,03: на порядок ниже порога, который видит глаз (~0,1 EV), и не больше
    /// 16 разных значений на весь диапазон ±0,5 EV. На телефоне (08c, `ae-trace`) подтяжка держалась в ±0,5 EV и возвращалась к 0–0,03 EV.
    fn default() -> Self {
        Self {
            metering: MeteringConfig::default(),
            controller: ExposureControllerConfig::default(),
            cold_start_every_frame_limit: 30,
            statistics_every_n_frames: 4,
            residual_gain_step_ev: 1.0 / 32.0,
            scene_change: SceneChangeConfig::default(),
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
    /// Статистику нельзя считать «каждый 0-й» кадр.
    #[error("viewfinder.auto_exposure.statistics_every_n_frames должен быть не меньше 1")]
    StatisticsEveryZero,
    /// Шаг квантования подтяжки не положителен или не конечен.
    #[error("viewfinder.auto_exposure.residual_gain_step_ev должен быть конечным и > 0, а не {0}")]
    ResidualGainStep(String),
    /// Настройки детектора смены сцены отвергнуты.
    #[error("viewfinder.auto_exposure.scene_change: {0}")]
    SceneChange(String),
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
        if self.statistics_every_n_frames == 0 {
            return Err(AutoExposureConfigError::StatisticsEveryZero);
        }
        if !(self.residual_gain_step_ev.is_finite() && self.residual_gain_step_ev > 0.0) {
            return Err(AutoExposureConfigError::ResidualGainStep(
                self.residual_gain_step_ev.to_string(),
            ));
        }
        self.scene_change
            .validate()
            .map_err(|error| AutoExposureConfigError::SceneChange(error.to_string()))?;
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
        assert_eq!(config.statistics_every_n_frames, 4, "каждый 4-й кадр: 15 Гц при 60 к/с");
    }

    #[test]
    fn a_zero_statistics_period_is_rejected_with_the_field_name() {
        let zero_period = AutoExposureConfig {
            statistics_every_n_frames: 0,
            ..AutoExposureConfig::default()
        };
        let error = zero_period.validate().expect_err("0 — не период");
        assert_eq!(error, AutoExposureConfigError::StatisticsEveryZero);
        assert!(
            error
                .to_string()
                .contains("viewfinder.auto_exposure.statistics_every_n_frames")
        );
        let parsed: AutoExposureConfig =
            serde_json::from_str(r#"{"statistics_every_n_frames":2}"#).expect("секция читается");
        assert_eq!(parsed.statistics_every_n_frames, 2);
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
        let mut broken_scene_change = AutoExposureConfig::default();
        broken_scene_change.scene_change.confirm_frames = 0;
        let error = broken_scene_change
            .validate()
            .expect_err("0 кадров подтверждения — не конфиг");
        assert!(matches!(error, AutoExposureConfigError::SceneChange(_)));
        assert!(
            error
                .to_string()
                .contains("viewfinder.auto_exposure.scene_change: поле scene_change.confirm_frames"),
            "{error}"
        );
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
        // Секция детектора смены сцены читается из конфига, остальное — по умолчанию.
        let parsed: AutoExposureConfig =
            serde_json::from_str(r#"{"scene_change":{"confirm_ms":800}}"#).expect("секция читается");
        assert_eq!(parsed.scene_change.confirm_ms, 800);
        assert_eq!(
            parsed.scene_change.changed_share,
            SceneChangeConfig::default().changed_share
        );
    }
}
