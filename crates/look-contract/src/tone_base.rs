//! Модель основы тона как часть рецепта пресета (решение 195).
//!
//! Основа (AgX / ACES / filmic / прежняя S-кривая) определяет, КАК сырой кадр превращается в картинку
//! до слоя вида. Раньше её выбирал только конфиг, но владелец крутит вид как «основа + ручки»: пресет,
//! собранный на ACES, на AgX выглядит иначе. Поэтому основа лежит в словаре вида и входит в каждый
//! пресет ([`crate::LookRecipe`]), а НЕ в [`crate::LookSettings`]: нейтраль настроек («чистый» вид) обязана
//! по-прежнему означать «текущая модель, ручки в нуле».
//!
//! Словарь не знает, как модель считается: формулы — в `isp-model`, соответствие — на стороне приложения.

use serde::{Deserialize, Serialize};

/// Модель основы тона. Имена в файле пресетов — `snake_case`: `"agx"`, `"aces"`, `"filmic"`, `"legacy"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneBaseModel {
    /// AgX.
    Agx,
    /// ACES.
    Aces,
    /// filmic (идея darktable filmic RGB).
    Filmic,
    /// Прежняя S-кривая.
    Legacy,
}

impl ToneBaseModel {
    /// Все модели в порядке показа на экране.
    pub const ALL: [ToneBaseModel; 4] = [
        ToneBaseModel::Agx,
        ToneBaseModel::Aces,
        ToneBaseModel::Filmic,
        ToneBaseModel::Legacy,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_snake_case_names_for_every_model() {
        let expected = ["\"agx\"", "\"aces\"", "\"filmic\"", "\"legacy\""];
        for (model, text) in ToneBaseModel::ALL.into_iter().zip(expected) {
            assert_eq!(serde_json::to_string(&model).unwrap(), text);
            assert_eq!(serde_json::from_str::<ToneBaseModel>(text).unwrap(), model);
        }
    }

    #[test]
    fn unknown_model_name_is_rejected() {
        assert!(serde_json::from_str::<ToneBaseModel>("\"Agx\"").is_err());
        assert!(serde_json::from_str::<ToneBaseModel>("\"reinhard\"").is_err());
    }
}
