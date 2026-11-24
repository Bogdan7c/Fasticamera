//! Секция `viewfinder.white_balance`: настройки нашего AWB на потоке рендера (сессия 08).
//!
//! Числа оценки света (таблица, оценщик, контроллер) живут в `scene-estimation`; здесь — то, что решает
//! поток рендера: сколько кадров нового потока прятать до первой оценки и как часто считать статистику.

use scene_estimation::config::SceneEstimationConfig;
use scene_estimation::white_source::phone::PhoneWhiteConfig;
use serde::{Deserialize, Serialize};

/// Настройки AWB потока рендера.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteBalanceConfig {
    /// Оценка света сцены: таблица локуса, оценщик, контроллер.
    pub scene: SceneEstimationConfig,
    /// Сколько кадров нового потока без прежней белой сцены обрабатываются, но НЕ показываются, пока нет первой
    /// оценки (решение 65). Предел обязателен: пустая или пересвеченная сцена не должна прятать картинку вечно.
    pub max_hidden_frames: u32,
    /// Статистика AWB на GPU считается на каждом N-м обработанном кадре (1 — на каждом).
    pub statistics_every_n_frames: u32,
    /// Числа источника белого «телефон» (white-balance 01): когда подсказки автоматики телефона считаются ответом.
    pub phone: PhoneWhiteConfig,
}

impl Default for WhiteBalanceConfig {
    /// * `max_hidden_frames` 20 ≈ 330 мс при 60 к/с (решение владельца 76, обновляет 65). Статистика читается
    ///   после fence слота (при `frames_in_flight = 2` оценка готова на 3-м кадре), значит нормальный старт прячет
    ///   2–3 кадра. Но на холодном старте камеры экспозиция стартовая (4 с·ISO): кадр пересвечен или тёмен, AWB
    ///   его не оценивает, и оценка приходит только когда наш AE сведёт экспозицию (замер + задержка сенсора
    ///   7–8 кадров ≈ 10–12 кадров). 20 кадров — запас, чтобы лимит не срабатывал раньше сходимости AE; ещё
    ///   ≤ 330 мс чёрного заметно меньше, чем цветной кадр с временной белой.
    ///   Оценка до замера на телефоне: реальное число скрытых кадров меряется в `hidden_frames_cold_start`.
    /// * `statistics_every_n_frames` 4 → 15 Гц при 60 к/с. Стадия статистики стоила ≈ 55 мА из ≈ 119 мА всего ISP
    ///   на телефоне (07a); свет сцены меняется медленнее (сглаживание ≥ 0,25 с), 15 Гц даёт не меньше трёх
    ///   оценок за постоянную времени самого быстрого профиля. Сглаживание при этом идёт на КАЖДЫЙ кадр.
    fn default() -> Self {
        Self {
            scene: SceneEstimationConfig::default(),
            max_hidden_frames: 20,
            statistics_every_n_frames: 4,
            phone: PhoneWhiteConfig::default(),
        }
    }
}

/// Что не так с секцией.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WhiteBalanceConfigError {
    /// Настройки оценки света отвергнуты.
    #[error("viewfinder.white_balance.scene: {0}")]
    Scene(String),
    /// Статистику нельзя считать «каждый 0-й» кадр.
    #[error("viewfinder.white_balance.statistics_every_n_frames должен быть не меньше 1")]
    StatisticsEveryZero,
    /// Числа источника «телефон» отвергнуты.
    #[error("viewfinder.white_balance.phone: {0}")]
    Phone(String),
}

impl WhiteBalanceConfig {
    /// Проверяет секцию.
    pub fn validate(&self) -> Result<(), WhiteBalanceConfigError> {
        self.scene
            .validate()
            .map_err(|error| WhiteBalanceConfigError::Scene(error.to_string()))?;
        if self.statistics_every_n_frames == 0 {
            return Err(WhiteBalanceConfigError::StatisticsEveryZero);
        }
        self.phone
            .validate()
            .map_err(|error| WhiteBalanceConfigError::Phone(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_decimate_to_15_hz() {
        let config = WhiteBalanceConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!((config.max_hidden_frames, config.statistics_every_n_frames), (20, 4));
    }

    #[test]
    fn invalid_values_and_unknown_fields_are_rejected() {
        let zero_period = WhiteBalanceConfig {
            statistics_every_n_frames: 0,
            ..WhiteBalanceConfig::default()
        };
        assert_eq!(
            zero_period.validate(),
            Err(WhiteBalanceConfigError::StatisticsEveryZero)
        );
        let mut broken_scene = WhiteBalanceConfig::default();
        broken_scene.scene.estimator.min_level = 0.99;
        assert!(matches!(
            broken_scene.validate(),
            Err(WhiteBalanceConfigError::Scene(_))
        ));
        assert!(serde_json::from_str::<WhiteBalanceConfig>(r#"{"hidden":3}"#).is_err());
        let mut broken_phone = WhiteBalanceConfig::default();
        broken_phone.phone.stable_frames = 0;
        assert!(matches!(
            broken_phone.validate(),
            Err(WhiteBalanceConfigError::Phone(_))
        ));
        assert!(
            serde_json::from_str::<WhiteBalanceConfig>(r#"{"phone":{"nope":1}}"#).is_err(),
            "в секции phone неизвестные поля отвергаются"
        );
        let tuned: WhiteBalanceConfig =
            serde_json::from_str(r#"{"phone":{"stable_frames":3,"settle_by_frame":10}}"#).expect("секция читается");
        assert_eq!((tuned.phone.stable_frames, tuned.phone.settle_by_frame), (3, 10));
        assert_eq!(tuned.validate(), Ok(()));
        let parsed: WhiteBalanceConfig = serde_json::from_str(r#"{"max_hidden_frames":0}"#).expect("секция читается");
        assert_eq!(
            parsed.max_hidden_frames, 0,
            "0 — показывать сразу, без скрытия (для стендов)"
        );
    }
}
