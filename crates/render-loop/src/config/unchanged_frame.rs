//! Секция `unchanged_frame` конфига потока рендера: не рисовать и не показывать кадр, когда на экране
//! ничего бы не изменилось (решение 214, сессия 16a).
//!
//! Зачем: экран 120 Гц при камере 60 к/с получает тик каждые 8,3 мс, а новый кадр камеры — через тик.
//! Тик без нового кадра камеры раньше перерисовывал весь экран (компоновщик + UI со стеклом, 1,3–2,8 мс GPU
//! на Adreno 740) и показывал ту же картинку. Теперь, если список рисования UI и всё остальное, что влияет на
//! пиксели, совпадает с уже показанным кадром, тик не рисуется: экран продолжает держать прежний кадр.

use serde::{Deserialize, Serialize};

/// Наибольшая допустимая длина серии пропусков: больше — по сути «пропускать всегда».
pub const MAX_CONSECUTIVE_SKIPS_LIMIT: u32 = 1_000;

/// Что делать с тиком, на котором картинка экрана не изменилась.
///
/// В JSON значения пишутся `skip_when_unchanged` / `always_redraw`; для запуска с телефона принимаются и короткие
/// `skip` / `always` (intent-параметр `unchanged_frame`). Любое другое значение — отказ разбора конфига, и в
/// тексте ошибки названо поле `unchanged_frame.policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum UnchangedFramePolicy {
    /// Тик без изменений не рисуется и не показывается: экран держит прежний кадр. Включается явно.
    SkipWhenUnchanged,
    /// Каждый тик рисуется и показывается, как до решения 214. Умолчание продукта (см. `Default` ниже). Нужен
    /// также для замера A/B (ток, GPU) и тестов, которым важен каждый показ.
    // УКАЗАТЕЛЬ ДЛЯ ОРКЕСТРАТОРА (решение 214, сессия 16a): умолчание намеренно `AlwaysRedraw` — выигрыш пропуска
    // ещё не измерен на телефоне (ток A/B с владельцем), а в продукт по умолчанию входит только доказанная замером
    // оптимизация. Переключение на `SkipWhenUnchanged` — одна строка здесь, после замера.
    #[default]
    AlwaysRedraw,
}

impl TryFrom<String> for UnchangedFramePolicy {
    type Error = String;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        match name.trim() {
            "skip_when_unchanged" | "skip" => Ok(Self::SkipWhenUnchanged),
            "always_redraw" | "always" => Ok(Self::AlwaysRedraw),
            other => Err(format!(
                "unchanged_frame.policy: неизвестное значение {other:?}, ожидалось skip_when_unchanged | always_redraw (или skip | always)"
            )),
        }
    }
}

impl From<UnchangedFramePolicy> for String {
    fn from(policy: UnchangedFramePolicy) -> Self {
        match policy {
            UnchangedFramePolicy::SkipWhenUnchanged => "skip_when_unchanged",
            UnchangedFramePolicy::AlwaysRedraw => "always_redraw",
        }
        .to_owned()
    }
}

/// Секция `unchanged_frame`; отсутствующие поля — из [`Default`], неизвестные — ошибка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UnchangedFrameConfig {
    /// Пропускать ли тики без изменений.
    pub policy: UnchangedFramePolicy,
    /// Сколько тиков подряд можно пропустить, прежде чем кадр всё равно нарисуется. Итоги слота кольца кадров
    /// (время GPU, статистика AWB, освобождение импортов и лизингов камеры) собираются только когда слот выдан
    /// заново, а пропущенный тик слот не берёт: без этого предела тихий экран при замершей камере не собирал бы
    /// итоги сколь угодно долго. 8 — камера 15 к/с на экране 120 Гц ещё укладывается без принудительной отрисовки.
    pub max_consecutive_skips: u32,
}

impl Default for UnchangedFrameConfig {
    fn default() -> Self {
        Self {
            policy: UnchangedFramePolicy::default(),
            max_consecutive_skips: 8,
        }
    }
}

impl UnchangedFrameConfig {
    /// Проверяет числа секции.
    pub fn validate(&self) -> Result<(), UnchangedFrameConfigError> {
        if self.max_consecutive_skips == 0 || self.max_consecutive_skips > MAX_CONSECUTIVE_SKIPS_LIMIT {
            return Err(UnchangedFrameConfigError::ConsecutiveSkipsOutOfRange {
                value: self.max_consecutive_skips,
            });
        }
        Ok(())
    }
}

/// Что не так в секции `unchanged_frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnchangedFrameConfigError {
    /// Серия пропусков должна быть от 1 до [`MAX_CONSECUTIVE_SKIPS_LIMIT`].
    #[error("unchanged_frame.max_consecutive_skips = {value}: нужно от 1 до {MAX_CONSECUTIVE_SKIPS_LIMIT}")]
    ConsecutiveSkipsOutOfRange { value: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_redraws_every_tick_until_the_skip_is_measured_and_is_valid() {
        let config = UnchangedFrameConfig::default();
        assert_eq!(config.policy, UnchangedFramePolicy::AlwaysRedraw);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn broken_run_limit_is_rejected_at_both_ends() {
        for value in [0, MAX_CONSECUTIVE_SKIPS_LIMIT + 1] {
            let config = UnchangedFrameConfig {
                max_consecutive_skips: value,
                ..UnchangedFrameConfig::default()
            };
            assert_eq!(
                config.validate(),
                Err(UnchangedFrameConfigError::ConsecutiveSkipsOutOfRange { value })
            );
        }
    }

    #[test]
    fn section_reads_from_json_and_rejects_unknown_keys() {
        let config: UnchangedFrameConfig = serde_json::from_str(r#"{"policy": "skip_when_unchanged"}"#).unwrap();
        assert_eq!(config.policy, UnchangedFramePolicy::SkipWhenUnchanged);
        assert_eq!(
            config.max_consecutive_skips,
            UnchangedFrameConfig::default().max_consecutive_skips,
            "отсутствующее поле — из умолчаний"
        );
        assert!(serde_json::from_str::<UnchangedFrameConfig>(r#"{"policies": "x"}"#).is_err());
    }

    #[test]
    fn short_launch_names_are_accepted_and_a_wrong_value_names_the_field() {
        for (text, expected) in [
            ("skip", UnchangedFramePolicy::SkipWhenUnchanged),
            ("always", UnchangedFramePolicy::AlwaysRedraw),
        ] {
            let config: UnchangedFrameConfig = serde_json::from_str(&format!(r#"{{"policy": "{text}"}}"#)).unwrap();
            assert_eq!(config.policy, expected, "{text}");
        }
        let error = serde_json::from_str::<UnchangedFrameConfig>(r#"{"policy": "sometimes"}"#)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unchanged_frame.policy") && error.contains("sometimes"),
            "{error}"
        );
        // Круговой обмен: запись и чтение дают то же значение.
        let written = serde_json::to_string(&UnchangedFrameConfig {
            policy: UnchangedFramePolicy::SkipWhenUnchanged,
            ..UnchangedFrameConfig::default()
        })
        .unwrap();
        assert!(written.contains("skip_when_unchanged"), "{written}");
    }
}
