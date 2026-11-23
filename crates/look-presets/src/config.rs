//! Конфиг хранилища пресетов: имена, файл, значения встроенного «Как глаз».
//!
//! Всё, что владелец может захотеть подкрутить без пересборки логики, живёт здесь.
//! Стартовые числа помечены `[estimate]`: подбирает владелец глазами в 14b.

use look_contract::{
    BuiltinPreset, HueBandId, LookKnob, LookKnobError, LookRecipe, LookSettings, PresetName, PresetNameError,
    ToneBaseModel,
};
use serde::{Deserialize, Serialize};

/// Ошибка проверки конфига; в тексте всегда есть имя поля.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// Имя файла пусто, содержит разделитель пути или равно «.» / «..».
    #[error("file_name: недопустимое имя файла {value:?} (нужно простое имя без каталогов)")]
    InvalidFileName {
        /// Что пришло.
        value: String,
    },
    /// Суффикс временного файла пуст: временный файл совпал бы с основным.
    #[error("temp_file_suffix: суффикс временного файла не должен быть пустым")]
    EmptyTempSuffix,
    /// Отображаемые имена встроенных совпали без учёта регистра.
    #[error("builtin_names: имена встроенных пресетов совпадают без учёта регистра: {value:?}")]
    DuplicateBuiltinNames {
        /// Совпавшее имя.
        value: String,
    },
    /// Отображаемое имя встроенного не прошло проверку имени пресета.
    #[error("builtin_names.{field}: {source}")]
    InvalidBuiltinName {
        /// Поле конфига.
        field: &'static str,
        /// Что не так с именем.
        source: PresetNameError,
    },
    /// Значение ручки «Как глаз» вне [−1, 1] или не число.
    #[error("as_eye.{field}: {source}")]
    InvalidKnob {
        /// Поле конфига.
        field: &'static str,
        /// Что не так со значением.
        source: LookKnobError,
    },
    /// Размер очереди исходов записи равен нулю.
    #[error("outcome_queue_capacity: должен быть не меньше 1")]
    ZeroOutcomeQueue,
}

/// Отображаемые имена встроенных пресетов (английские: тексты UI английские).
/// Строки проверяются в [`PresetStoreConfig::builtin_catalog`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BuiltinNames {
    /// Имя «Честного» пресета. `[estimate]`
    pub honest: String,
    /// Имя пресета «Как глаз». `[estimate]`
    pub as_eye: String,
}

impl Default for BuiltinNames {
    fn default() -> Self {
        Self {
            honest: "Honest".to_owned(),
            as_eye: "Natural".to_owned(),
        }
    }
}

/// Значения ручек встроенного «Как глаз» (без полос). Числа, не `LookKnob`: проверка
/// диапазона идёт в [`PresetStoreConfig::builtin_catalog`] с именем поля в ошибке.
///
/// `[estimate]` Умеренный «живой» вид: контраст +0.2 (чуть плотнее середина, как её видит
/// глаз при дневном свете), насыщенность +0.15 и сочность +0.15 (цвет заметнее, но без
/// пластика, решение 97). Остальное в нуле: теплоту, тени и света владелец крутит сам.
/// Окончательный подбор — в 14b.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AsEyeValues {
    /// Контраст.
    pub contrast: f32,
    /// Средние тона.
    pub midtones: f32,
    /// Тени.
    pub shadows: f32,
    /// Света.
    pub highlights: f32,
    /// Насыщенность.
    pub saturation: f32,
    /// Сочность.
    pub vibrance: f32,
    /// Теплота.
    pub warmth: f32,
    /// Оттенок.
    pub tint: f32,
}

impl Default for AsEyeValues {
    fn default() -> Self {
        Self {
            contrast: 0.2,
            midtones: 0.0,
            shadows: 0.0,
            highlights: 0.0,
            saturation: 0.15,
            vibrance: 0.15,
            warmth: 0.0,
            tint: 0.0,
        }
    }
}

impl AsEyeValues {
    /// Проверяет каждое значение и собирает настройки вида.
    fn to_settings(&self) -> Result<LookSettings, ConfigError> {
        let knob = |field: &'static str, value: f32| {
            LookKnob::new(value).map_err(|source| ConfigError::InvalidKnob { field, source })
        };
        Ok(LookSettings {
            contrast: knob("contrast", self.contrast)?,
            midtones: knob("midtones", self.midtones)?,
            shadows: knob("shadows", self.shadows)?,
            highlights: knob("highlights", self.highlights)?,
            saturation: knob("saturation", self.saturation)?,
            vibrance: knob("vibrance", self.vibrance)?,
            warmth: knob("warmth", self.warmth)?,
            tint: knob("tint", self.tint)?,
            ..LookSettings::NEUTRAL
        })
    }
}

/// То, что каталог узнаёт не из своего конфига, а от остальной системы (решение 195): у хранилища
/// нет права решать, какая основа тона «по умолчанию» и какие полосы оттенка существуют, это знают
/// конфиг рендера и модель вида. Структура, а не позиционные аргументы, чтобы вызов читался сам.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEnvironment {
    /// Основа тона встроенных пресетов («Честный», «Как глаз»): берётся из конфига рендера.
    pub default_base: ToneBaseModel,
    /// Идентификаторы полос оттенка модели вида: по ним отличается «нет такой полосы» от «полоса не тронута».
    pub band_ids: Vec<HueBandId>,
}

/// Конфиг хранилища пресетов.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PresetStoreConfig {
    /// Имя файла пресетов внутри папки приложения.
    pub file_name: String,
    /// Суффикс временного файла при атомарной записи (`<file_name><суффикс>`).
    pub temp_file_suffix: String,
    /// Отображаемые имена встроенных пресетов.
    pub builtin_names: BuiltinNames,
    /// Значения встроенного «Как глаз». «Честный» всегда нейтраль и сюда не входит.
    pub as_eye: AsEyeValues,
    /// Какой пресет активен при отсутствии файла и после удаления активного (решение 98).
    pub startup_preset: BuiltinPreset,
    /// Сколько исходов записи хранить для опроса (старые вытесняются).
    /// `[estimate]` 32: экран опрашивает исходы часто, переполнения не ожидается.
    pub outcome_queue_capacity: usize,
}

impl Default for PresetStoreConfig {
    fn default() -> Self {
        Self {
            file_name: "look-presets.json".to_owned(),
            temp_file_suffix: ".tmp".to_owned(),
            builtin_names: BuiltinNames::default(),
            as_eye: AsEyeValues::default(),
            startup_preset: BuiltinPreset::AsEye,
            outcome_queue_capacity: 32,
        }
    }
}

impl PresetStoreConfig {
    /// Проверяет конфиг; ошибка называет поле.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let name = self.file_name.as_str();
        let looks_like_path = name.contains('/') || name.contains('\\');
        if name.is_empty() || name == "." || name == ".." || looks_like_path {
            return Err(ConfigError::InvalidFileName {
                value: self.file_name.clone(),
            });
        }
        if self.temp_file_suffix.is_empty() {
            return Err(ConfigError::EmptyTempSuffix);
        }
        if self.outcome_queue_capacity == 0 {
            return Err(ConfigError::ZeroOutcomeQueue);
        }
        Ok(())
    }

    /// Проверяет конфиг и собирает каталог встроенных пресетов (имена и ручки тоже проверяются).
    pub fn builtin_catalog(&self, environment: &CatalogEnvironment) -> Result<BuiltinCatalog, ConfigError> {
        self.validate()?;
        let honest_name =
            PresetName::new(&self.builtin_names.honest).map_err(|source| ConfigError::InvalidBuiltinName {
                field: "honest",
                source,
            })?;
        let as_eye_name =
            PresetName::new(&self.builtin_names.as_eye).map_err(|source| ConfigError::InvalidBuiltinName {
                field: "as_eye",
                source,
            })?;
        if honest_name.same_name_as(as_eye_name.as_str()) {
            return Err(ConfigError::DuplicateBuiltinNames {
                value: honest_name.as_str().to_owned(),
            });
        }
        Ok(BuiltinCatalog {
            honest_name,
            as_eye_name,
            as_eye_settings: self.as_eye.to_settings()?,
            startup_preset: self.startup_preset,
            default_base: environment.default_base,
            band_ids: environment.band_ids.clone(),
        })
    }
}

/// Проверенный каталог встроенных пресетов: имена и значения. Получить можно только из
/// проверенного конфига, поэтому библиотека не перепроверяет его.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltinCatalog {
    honest_name: PresetName,
    as_eye_name: PresetName,
    as_eye_settings: LookSettings,
    startup_preset: BuiltinPreset,
    default_base: ToneBaseModel,
    band_ids: Vec<HueBandId>,
}

impl BuiltinCatalog {
    /// Отображаемое имя встроенного.
    pub fn name_of(&self, preset: BuiltinPreset) -> &PresetName {
        match preset {
            BuiltinPreset::Honest => &self.honest_name,
            BuiltinPreset::AsEye => &self.as_eye_name,
        }
    }

    /// Значения ручек встроенного. «Честный» — всегда нейтраль, не из конфига.
    pub fn settings_of(&self, preset: BuiltinPreset) -> LookSettings {
        match preset {
            BuiltinPreset::Honest => LookSettings::NEUTRAL,
            BuiltinPreset::AsEye => self.as_eye_settings.clone(),
        }
    }

    /// Полный рецепт встроенного: основа по умолчанию из конфига рендера + значения ручек (решение 195).
    pub fn recipe_of(&self, preset: BuiltinPreset) -> LookRecipe {
        LookRecipe {
            base: self.default_base,
            settings: self.settings_of(preset),
        }
    }

    /// Основа тона встроенных пресетов.
    pub fn default_base(&self) -> ToneBaseModel {
        self.default_base
    }

    /// Есть ли полоса оттенка с таким идентификатором в модели вида.
    pub fn knows_band(&self, band: &HueBandId) -> bool {
        self.band_ids.contains(band)
    }

    /// Пресет по умолчанию: активен без файла и после удаления активного своего.
    pub fn startup_preset(&self) -> BuiltinPreset {
        self.startup_preset
    }

    /// Занято ли имя отображаемым именем какого-либо встроенного (без учёта регистра).
    pub fn is_builtin_name(&self, candidate: &PresetName) -> bool {
        candidate.same_name_as(self.honest_name.as_str()) || candidate.same_name_as(self.as_eye_name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment() -> CatalogEnvironment {
        CatalogEnvironment {
            default_base: ToneBaseModel::Aces,
            band_ids: vec![HueBandId::new("sky").unwrap()],
        }
    }

    #[test]
    fn default_config_is_valid_with_expected_builtin_values() {
        let config = PresetStoreConfig::default();
        assert_eq!(config.validate(), Ok(()));
        let catalog = config.builtin_catalog(&environment()).unwrap();
        assert_eq!(catalog.name_of(BuiltinPreset::Honest).as_str(), "Honest");
        assert_eq!(catalog.name_of(BuiltinPreset::AsEye).as_str(), "Natural");
        assert!(catalog.settings_of(BuiltinPreset::Honest).is_neutral());
        let as_eye = catalog.settings_of(BuiltinPreset::AsEye);
        assert_eq!(as_eye.contrast.get(), 0.2);
        assert_eq!(as_eye.saturation.get(), 0.15);
        assert_eq!(as_eye.vibrance.get(), 0.15);
        assert_eq!(catalog.startup_preset(), BuiltinPreset::AsEye);
    }

    #[test]
    fn builtin_recipes_take_base_from_environment_and_settings_from_config() {
        let catalog = PresetStoreConfig::default().builtin_catalog(&environment()).unwrap();
        assert_eq!(catalog.default_base(), ToneBaseModel::Aces);
        for preset in [BuiltinPreset::Honest, BuiltinPreset::AsEye] {
            let recipe = catalog.recipe_of(preset);
            assert_eq!(recipe.base, ToneBaseModel::Aces, "{preset:?}");
            assert_eq!(recipe.settings, catalog.settings_of(preset), "{preset:?}");
        }
        assert!(catalog.recipe_of(BuiltinPreset::Honest).settings.is_neutral());
    }

    #[test]
    fn catalog_knows_only_bands_from_environment() {
        let catalog = PresetStoreConfig::default().builtin_catalog(&environment()).unwrap();
        assert!(catalog.knows_band(&HueBandId::new("sky").unwrap()));
        assert!(!catalog.knows_band(&HueBandId::new("ghost").unwrap()));
    }

    #[test]
    fn validate_names_the_bad_field() {
        let bad_file = PresetStoreConfig {
            file_name: "../x".into(),
            ..PresetStoreConfig::default()
        };
        assert!(matches!(bad_file.validate(), Err(ConfigError::InvalidFileName { .. })));
        let bad_suffix = PresetStoreConfig {
            temp_file_suffix: String::new(),
            ..PresetStoreConfig::default()
        };
        assert_eq!(bad_suffix.validate(), Err(ConfigError::EmptyTempSuffix));
        let zero_queue = PresetStoreConfig {
            outcome_queue_capacity: 0,
            ..PresetStoreConfig::default()
        };
        assert_eq!(zero_queue.validate(), Err(ConfigError::ZeroOutcomeQueue));
        let same_names = PresetStoreConfig {
            builtin_names: BuiltinNames {
                honest: "Same".into(),
                as_eye: " SAME ".into(),
            },
            ..PresetStoreConfig::default()
        };
        assert!(matches!(
            same_names.builtin_catalog(&environment()),
            Err(ConfigError::DuplicateBuiltinNames { .. })
        ));
        let empty_name = PresetStoreConfig {
            builtin_names: BuiltinNames {
                honest: "  ".into(),
                ..BuiltinNames::default()
            },
            ..PresetStoreConfig::default()
        };
        let error = empty_name.builtin_catalog(&environment()).unwrap_err();
        assert!(error.to_string().contains("builtin_names.honest"), "{error}");
        let bad_knob = PresetStoreConfig {
            as_eye: AsEyeValues {
                tint: 2.0,
                ..AsEyeValues::default()
            },
            ..PresetStoreConfig::default()
        };
        let error = bad_knob.builtin_catalog(&environment()).unwrap_err();
        assert!(error.to_string().contains("as_eye.tint"), "{error}");
        assert!(
            ConfigError::ZeroOutcomeQueue
                .to_string()
                .contains("outcome_queue_capacity")
        );
    }

    #[test]
    fn config_rejects_unknown_fields_and_accepts_partial() {
        assert!(serde_json::from_str::<PresetStoreConfig>(r#"{"nope":1}"#).is_err());
        let partial: PresetStoreConfig = serde_json::from_str(r#"{"file_name":"x.json"}"#).unwrap();
        assert_eq!(partial.file_name, "x.json");
        assert_eq!(partial.outcome_queue_capacity, 32);
    }

    #[test]
    fn builtin_names_are_reported_as_taken_in_any_case() {
        let catalog = PresetStoreConfig::default().builtin_catalog(&environment()).unwrap();
        assert!(catalog.is_builtin_name(&PresetName::new("honest").unwrap()));
        assert!(catalog.is_builtin_name(&PresetName::new("NATURAL").unwrap()));
        assert!(!catalog.is_builtin_name(&PresetName::new("Sunset").unwrap()));
    }
}
