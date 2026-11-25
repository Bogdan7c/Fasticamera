//! Секция `viewfinder.white_balance`: настройки нашего AWB на потоке рендера (сессия 08).
//!
//! Здесь — то, что решает поток рендера: сколько кадров нового потока прятать до ответа источника белого. Период статистики кадра — в
//! `viewfinder.auto_exposure` (статистика нужна AE).

use scene_estimation::white_source::phone::PhoneWhiteConfig;
use serde::{Deserialize, Serialize};

/// Настройки AWB потока рендера.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteBalanceConfig {
    /// Сколько кадров нового потока без прежней белой сцены обрабатываются, но НЕ показываются, пока нет первой
    /// оценки (решение 65). Предел обязателен: пустая или пересвеченная сцена не должна прятать картинку вечно.
    pub max_hidden_frames: u32,
    /// Показание источников белого (что источник видит сейчас) обновляется на каждом N-м годном кадре (1 — на каждом).
    pub reading_every_frames: u32,
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
    /// * `reading_every_frames` 15 → 4 раза в секунду при 60 к/с: число на кнопке источника не должно мельтешить
    ///   (автоматика телефона дрожит каждый кадр), а человеку хватает чувствовать свет с запаздыванием в четверть секунды.
    fn default() -> Self {
        Self {
            max_hidden_frames: 20,
            reading_every_frames: 15,
            phone: PhoneWhiteConfig::default(),
        }
    }
}

/// Что не так с секцией.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WhiteBalanceConfigError {
    /// Показание нельзя обновлять «каждый 0-й» кадр.
    #[error("viewfinder.white_balance.reading_every_frames должен быть не меньше 1")]
    ReadingEveryZero,
    /// Числа источника «телефон» отвергнуты.
    #[error("viewfinder.white_balance.phone: {0}")]
    Phone(String),
}

impl WhiteBalanceConfig {
    /// Проверяет секцию.
    pub fn validate(&self) -> Result<(), WhiteBalanceConfigError> {
        if self.reading_every_frames == 0 {
            return Err(WhiteBalanceConfigError::ReadingEveryZero);
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
    fn defaults_are_valid() {
        let config = WhiteBalanceConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.max_hidden_frames, 20);
    }

    #[test]
    fn the_statistics_period_moved_to_auto_exposure_and_the_old_name_is_rejected() {
        let error = serde_json::from_str::<WhiteBalanceConfig>(r#"{"statistics_every_n_frames":4}"#)
            .expect_err("поле переехало в viewfinder.auto_exposure");
        assert!(error.to_string().contains("statistics_every_n_frames"), "{error}");
    }

    #[test]
    fn invalid_values_and_unknown_fields_are_rejected() {
        let zero_reading = WhiteBalanceConfig {
            reading_every_frames: 0,
            ..WhiteBalanceConfig::default()
        };
        assert_eq!(zero_reading.validate(), Err(WhiteBalanceConfigError::ReadingEveryZero));
        assert_eq!(WhiteBalanceConfig::default().reading_every_frames, 15);
        // Секция `scene` (оценщик света) ушла вместе со старым оценщиком: старый конфиг с ней отвергается.
        assert!(serde_json::from_str::<WhiteBalanceConfig>(r#"{"scene":{}}"#).is_err());
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
