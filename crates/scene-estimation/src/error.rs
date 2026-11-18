//! Ошибки crate: конфиг, таблица локуса, оценщик.

use isp_model::color::dng::ColorProfileError;
use isp_model::color::white_point::WhitePointError;

/// Конфиг не прошёл проверку.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// Значение поля вне допустимого диапазона.
    #[error("поле {field}: {reason}")]
    Invalid {
        /// Путь поля в конфиге.
        field: &'static str,
        /// Что не так.
        reason: String,
    },
}

/// Таблицу локуса камеры построить не удалось.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LocusTableError {
    /// Конфиг таблицы неверен.
    #[error("конфиг таблицы: {0}")]
    Config(#[from] ConfigError),
    /// Белую точку не удалось перевести в цветность (температура вне таблицы Робертсона).
    #[error("белая точка {kelvin} К: {source}")]
    WhitePoint {
        /// Температура точки, К.
        kelvin: f64,
        /// Причина.
        source: WhitePointError,
    },
    /// Профиль цвета камеры не дал нейтраль для белой.
    #[error("нейтраль камеры для {kelvin} К: {source}")]
    Neutral {
        /// Температура точки, К.
        kelvin: f64,
        /// Причина.
        source: ColorProfileError,
    },
    /// Отрезок локуса вырожден (направление совпало с направлением Duv): положение по нему не определить.
    #[error("отрезок {index} локуса вырожден в пространстве камеры")]
    DegenerateSegment {
        /// Номер отрезка.
        index: usize,
    },
    /// Полоса допустимых белых не построилась.
    #[error("полоса локуса: {0}")]
    Band(WhitePointError),
}

/// Оценщик не смог выдать оценку по статистике.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EstimationError {
    /// Результат не переводится в цветность (вне таблицы Робертсона) — бывает только при битой таблице.
    #[error("оценка не переводится в цветность: {0}")]
    WhitePoint(#[from] WhitePointError),
    /// Конфиг оценщика неверен.
    #[error("конфиг оценщика: {0}")]
    Config(#[from] ConfigError),
}
