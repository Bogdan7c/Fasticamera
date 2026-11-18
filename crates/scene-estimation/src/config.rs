//! Настройки оценки света сцены. Все числа — конфиг; значения по умолчанию помечены источником.
//!
//! Принцип: «оценка» — значение, взятое до серой карты и телефона; уточняется в 09 (серая карта) и
//! сверкой с iOS. Поэтому пороги серой полосы намеренно широкие (см. [`EstimatorConfig::gray_band_duv`]).

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

/// Диапазон и детализация таблицы локуса камеры.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LocusTableConfig {
    /// Самая тёплая температура таблицы, К. Свет холоднее — «вне таблицы».
    pub min_temperature_kelvin: f64,
    /// Самая холодная (синяя) температура таблицы, К.
    pub max_temperature_kelvin: f64,
    /// Число отрезков (точек на одну больше). Равномерно по майредам.
    pub segments: u32,
    /// Смещение по Duv, на котором снимается направление «Duv» в пространстве камеры.
    pub duv_probe: f64,
}

/// Верхняя граница числа отрезков: цикл шейдера по отрезкам фиксирован, массив ограничен.
pub const MAX_LOCUS_SEGMENTS: u32 = 256;

impl Default for LocusTableConfig {
    /// 2000–12000 К покрывает свечу/лампу накаливания (≈ 1900–2800 К) и тень в ясный день (до ≈ 10000 К);
    /// 64 отрезка по майредам дают шаг ≈ 6,5 майред (глаз различает ≈ 5–10), оценка — по заданию 08.
    /// Проба Duv = 0,01: локус кусочно-линеен по Duv в пределах ±0,05, шаг не влияет на результат.
    fn default() -> Self {
        Self {
            min_temperature_kelvin: 2000.0,
            max_temperature_kelvin: 12000.0,
            segments: 64,
            duv_probe: 0.01,
        }
    }
}

impl LocusTableConfig {
    /// Проверка: конечные положительные температуры по порядку, число отрезков в пределах.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let finite = self.min_temperature_kelvin.is_finite() && self.max_temperature_kelvin.is_finite();
        if !finite || self.min_temperature_kelvin <= 0.0 || self.min_temperature_kelvin >= self.max_temperature_kelvin {
            return Err(ConfigError::Invalid {
                field: "locus.temperature",
                reason: "нужны конечные 0 < min < max".to_owned(),
            });
        }
        if !(2..=MAX_LOCUS_SEGMENTS).contains(&self.segments) {
            return Err(ConfigError::Invalid {
                field: "locus.segments",
                reason: format!("нужно 2…{MAX_LOCUS_SEGMENTS}, задано {}", self.segments),
            });
        }
        if !(self.duv_probe.is_finite() && self.duv_probe > 0.0 && self.duv_probe <= 0.05) {
            return Err(ConfigError::Invalid {
                field: "locus.duv_probe",
                reason: "нужно (0, 0,05]".to_owned(),
            });
        }
        Ok(())
    }
}

/// Как сводятся плитки-кандидаты в одну оценку.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum ReductionConfig {
    /// Взвешенное среднее (майред, Duv) всех кандидатов.
    WeightedMean,
    /// Сначала гистограмма весов по майредам и поиск пика в окне из трёх корзин, затем взвешенное среднее
    /// только кандидатов окна. Устойчиво к цветным поверхностям «около кривой» (кожа, дерево выглядят
    /// как более тёплый «серый», но у каждой такой поверхности свой майред, а у серых он один).
    ModeWindow {
        /// Число корзин гистограммы по майредам таблицы (3…[`MAX_HISTOGRAM_BINS`]).
        histogram_bins: u32,
    },
}

/// Верхняя граница числа корзин (в шейдере — массив в shared-памяти).
pub const MAX_HISTOGRAM_BINS: u32 = 128;

/// Настройки оценщика плиток.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EstimatorConfig {
    /// Плитка — кандидат в «серое», если её |Duv| от кривой Планка не больше этого. ДЕРЖАТЬ ШИРЕ ≈ 0,02:
    /// сверка нейтрали с эталоном на кадре «лампа» показала, что вендорский профиль цвета ≈ на 10 % мимо
    /// (`isp-model/src/color/dng_neutral_tests.rs`); сузить после прогона по серой карте (09).
    pub gray_band_duv: f32,
    /// Мягкая «тяга» к кривой Планка: вес кандидата в редукции умножается на `exp(−½·(Duv/σ)²)`, σ — это поле.
    /// Серые поверхности естественных источников лежат у кривой (|Duv| ≲ 0,01), цветные — дальше; жёсткий порог
    /// `gray_band_duv` остаётся широким из-за ошибки профиля, а тяга не даёт зелени/коже с |Duv| 0,02–0,04
    /// перебить нейтральные плитки числом. Тяга входит и в уверенность: однотонная зелень с |Duv| ≈ 0,03–0,04 проходит жёсткий порог, но уверенность её почти нулевая.
    pub duv_pull_sigma: f32,
    /// Доля непересвеченных ячеек плитки, ниже которой плитка не берётся (из `used / (used + saturated)`).
    pub min_used_fraction: f32,
    /// Нижняя граница освещённости плитки (средний G в долях белого, ДО баланса белого): темнее — шум.
    pub min_level: f32,
    /// Верхняя граница освещённости: ярче — колено насыщения сенсора искажает цвет.
    pub max_level: f32,
    /// Освещённость, с которой вес плитки полный; от `min_level` до неё вес растёт линейно.
    pub full_weight_level: f32,
    /// Способ свести кандидатов.
    pub reduction: ReductionConfig,
    /// Допустимое |Duv| итоговой оценки: AWB уходит от кривой Планка лишь в узкой полосе.
    /// 0,02 — примерно предел естественных источников (лампы дневного света, LED ±0,01…0,02).
    pub result_max_abs_duv: f64,
}

impl Default for EstimatorConfig {
    /// Все значения — оценка до серой карты (09):
    /// * `gray_band_duv` 0,04 — вдвое шире рабочей 0,02 из-за ≈ 10 % ошибки вендорского профиля;
    /// * `min_used_fraction` 0,5 — плитка с наполовину пересвеченными ячейками цвет уже искажает;
    /// * `min_level` 0,03 — ниже шум чтения заметен (≈ 3 % белого);
    /// * `max_level` 0,9 — ниже порога пересвета ячеек 0,95 с запасом на среднее по плитке;
    /// * `full_weight_level` 0,15 — к середине линейной шкалы шум уже мал;
    /// * `reduction` — режим окна по моде: на тесте ColorChecker (`tests/scene_reference.rs`) простое среднее
    ///   смещалось в тёплую сторону кожей и деревом.
    fn default() -> Self {
        Self {
            gray_band_duv: 0.04,
            duv_pull_sigma: 0.01,
            min_used_fraction: 0.5,
            min_level: 0.03,
            max_level: 0.9,
            full_weight_level: 0.15,
            reduction: ReductionConfig::ModeWindow { histogram_bins: 48 },
            result_max_abs_duv: 0.02,
        }
    }
}

impl EstimatorConfig {
    /// Проверка диапазонов.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let unit = |value: f32| value.is_finite() && value > 0.0 && value <= 1.0;
        if !(self.gray_band_duv.is_finite() && self.gray_band_duv > 0.0 && self.gray_band_duv <= 0.2) {
            return Err(invalid("estimator.gray_band_duv", "нужно (0, 0,2]"));
        }
        if !(self.duv_pull_sigma.is_finite() && self.duv_pull_sigma > 0.0) {
            return Err(invalid("estimator.duv_pull_sigma", "нужно конечное > 0"));
        }
        if !unit(self.min_used_fraction) {
            return Err(invalid("estimator.min_used_fraction", "нужно (0, 1]"));
        }
        if !(unit(self.min_level) && unit(self.max_level) && unit(self.full_weight_level)) {
            return Err(invalid("estimator.levels", "уровни должны лежать в (0, 1]"));
        }
        if self.min_level >= self.max_level || self.full_weight_level < self.min_level {
            return Err(invalid(
                "estimator.levels",
                "нужно min_level < max_level и full_weight_level ≥ min_level",
            ));
        }
        if !(self.result_max_abs_duv.is_finite() && self.result_max_abs_duv >= 0.0) {
            return Err(invalid("estimator.result_max_abs_duv", "нужно конечное ≥ 0"));
        }
        if let ReductionConfig::ModeWindow { histogram_bins } = self.reduction
            && !(3..=MAX_HISTOGRAM_BINS).contains(&histogram_bins)
        {
            return Err(invalid("estimator.reduction.histogram_bins", "нужно 3…128"));
        }
        Ok(())
    }
}

/// Настройки решения AWB: принять ли оценку, как быстро двигаться, когда сообщать о смене белой.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ControllerConfig {
    /// Оценка с меньшей уверенностью не обновляет цель (белая держится).
    pub min_confidence: f32,
    /// Оценка с меньшим числом принятых плиток не обновляет цель.
    pub min_accepted_tiles: u32,
    /// Постоянная времени приближения профиля «Фото», с.
    pub photo_time_constant_seconds: f64,
    /// Постоянная времени профиля «Видео», с (медленно: без «дыхания»).
    pub video_time_constant_seconds: f64,
    /// Постоянная времени приближения к ручной точке белого, с (быстро: пользователь двигает ползунок).
    pub manual_time_constant_seconds: f64,
    /// Наибольший шаг времени между кадрами, с: больший скачок (смена потока, пауза) не «перематывает» сглаживание.
    pub max_step_seconds: f64,
    /// Белая сообщается ISP заново, лишь когда сдвинулась больше, чем на столько майред.
    pub report_threshold_mired: f64,
    /// … или больше, чем на столько Duv.
    pub report_threshold_duv: f64,
    /// Допустимое |Duv| ручной точки белого (в нём же — «ограничено до»).
    pub manual_max_abs_duv: f64,
}

impl Default for ControllerConfig {
    /// Все значения — оценка, сверить с 09 и поведением iOS:
    /// * уверенность 0,17 — между лучшей сценой без нейтральных фикстуры (0,119) и самой слабой сценой с
    ///   ColorChecker (0,220); однотонная зелень — ≤ 0,03; 12 плиток (≈ 1,5 % сетки 32×24): меньше — доминирует случайная поверхность;
    /// * «Фото» 0,25 с — быстро, за ≈ 0,6 с на 90 %; «Видео» 1,5 с — за ≈ 3,5 с, без заметных скачков;
    /// * ручная 0,10 с — отклик ползунка;
    /// * шаг ≤ 0,25 с (15 кадров 60 к/с): дольше — это пауза, не движение;
    /// * порог отчёта 0,5 майред и 0,0002 Duv — на порядок ниже заметной разницы (≈ 5 майред, ≈ 0,002).
    fn default() -> Self {
        Self {
            min_confidence: 0.17,
            min_accepted_tiles: 12,
            photo_time_constant_seconds: 0.25,
            video_time_constant_seconds: 1.5,
            manual_time_constant_seconds: 0.10,
            max_step_seconds: 0.25,
            report_threshold_mired: 0.5,
            report_threshold_duv: 0.0002,
            manual_max_abs_duv: 0.05,
        }
    }
}

impl ControllerConfig {
    /// Проверка диапазонов.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(self.min_confidence.is_finite() && (0.0..=1.0).contains(&self.min_confidence)) {
            return Err(invalid("controller.min_confidence", "нужно [0, 1]"));
        }
        let positive = |value: f64| value.is_finite() && value > 0.0;
        for (field, value) in [
            (
                "controller.photo_time_constant_seconds",
                self.photo_time_constant_seconds,
            ),
            (
                "controller.video_time_constant_seconds",
                self.video_time_constant_seconds,
            ),
            (
                "controller.manual_time_constant_seconds",
                self.manual_time_constant_seconds,
            ),
            ("controller.max_step_seconds", self.max_step_seconds),
            ("controller.report_threshold_mired", self.report_threshold_mired),
            ("controller.report_threshold_duv", self.report_threshold_duv),
        ] {
            if !positive(value) {
                return Err(invalid(field, "нужно конечное > 0"));
            }
        }
        if !(self.manual_max_abs_duv.is_finite() && self.manual_max_abs_duv >= 0.0) {
            return Err(invalid("controller.manual_max_abs_duv", "нужно конечное ≥ 0"));
        }
        Ok(())
    }
}

/// Полная конфигурация оценки света сцены.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SceneEstimationConfig {
    /// Таблица локуса камеры.
    pub locus: LocusTableConfig,
    /// Оценщик плиток.
    pub estimator: EstimatorConfig,
    /// Решение AWB.
    pub controller: ControllerConfig,
}

impl SceneEstimationConfig {
    /// Проверяет все секции.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.locus.validate()?;
        self.estimator.validate()?;
        self.controller.validate()
    }
}

fn invalid(field: &'static str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        field,
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(SceneEstimationConfig::default().validate(), Ok(()));
    }

    #[test]
    fn unknown_field_is_rejected_and_missing_fields_use_defaults() {
        let parsed: Result<SceneEstimationConfig, _> = serde_json::from_str(r#"{"estimator":{"gray_band":0.1}}"#);
        assert!(parsed.is_err(), "неизвестное поле обязано отклоняться");
        let partial: SceneEstimationConfig =
            serde_json::from_str(r#"{"controller":{"min_confidence":0.3}}"#).expect("частичный конфиг");
        assert_eq!(partial.controller.min_confidence, 0.3);
        assert_eq!(partial.locus, LocusTableConfig::default());
    }

    #[test]
    fn invalid_values_name_the_field() {
        let mut config = SceneEstimationConfig::default();
        config.estimator.min_level = 0.95;
        let error = config.validate().expect_err("min_level ≥ max_level");
        assert!(error.to_string().contains("estimator.levels"), "{error}");
        let mut config = SceneEstimationConfig::default();
        config.locus.segments = 1;
        assert!(config.validate().is_err());
        let mut config = SceneEstimationConfig::default();
        config.controller.report_threshold_mired = 0.0;
        assert!(config.validate().is_err());
    }
}
