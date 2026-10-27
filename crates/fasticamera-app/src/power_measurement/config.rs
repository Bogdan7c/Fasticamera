//! Настройки замера тока (`camera.scenarios.power`, сессия 07, решение 45).
//!
//! Все числа, от которых зависит вывод «стадия стоит N мА» или «не различим», живут здесь, а не в
//! коде замера. Значения по умолчанию — из плана сессии 07 («Бюджет по стадиям»): окно 60 с, не
//! меньше трёх пар, десятиминутный прогон; остальное объяснено у каждого поля.

use std::num::NonZeroUsize;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shell_contract::power::{
    CurrentNowUnit, DischargeSign, PlausibleCurrentRange, PowerSamplingPeriod, PowerSamplingRequest, SignAgreement,
    SignDetectionRule,
};

/// Как установить знак тока разряда (`CURRENT_NOW` у вендоров бывает инвертирован).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DischargeSignSetting {
    /// Определить по показаниям первого окна (`detect_discharge_sign`); противоречие — отказ мерить.
    Detect,
    /// Как в документации Android: разряд отрицательный.
    NegativeWhenDischarging,
    /// Инвертировано вендором: разряд положительный.
    PositiveWhenDischarging,
}

impl DischargeSignSetting {
    /// Заданный явно знак; `None` — определять по показаниям.
    pub fn fixed(self) -> Option<DischargeSign> {
        match self {
            DischargeSignSetting::Detect => None,
            DischargeSignSetting::NegativeWhenDischarging => Some(DischargeSign::NegativeWhenDischarging),
            DischargeSignSetting::PositiveWhenDischarging => Some(DischargeSign::PositiveWhenDischarging),
        }
    }
}

/// A/B-замер вклада стадии.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AbExperimentConfig {
    /// Длина одного окна (A или B), мс. 60 с — решение 45: ток батареи сглажен контроллером
    /// заряда, короткие окна шумят сильнее, чем вклад лёгкой стадии.
    pub window_ms: u64,
    /// Сколько пар A/B. Решение 45: не меньше трёх, чтобы разброс между повторами был виден.
    /// Чётное число пар убирает линейный дрейф (нагрев, падение напряжения) из среднего ТОЧНО,
    /// нечётное — оставляет 1/N дрейфа за окно (см. `ab_experiment`).
    pub pairs: u32,
    /// Сколько первых миллисекунд каждого окна выбрасывается, мс: после переключения стадии ток
    /// и частоты GPU выходят на новый уровень не сразу. 10 с — оценка с запасом (DVFS телефона
    /// перестраивается за сотни миллисекунд, сглаживание тока у контроллера — секунды).
    pub warmup_ms: u64,
    /// Сколько ждать запоздавших показаний после конца окна, прежде чем его закрыть, мс.
    /// Показания забираются из кольца раз в тик маршрутизатора; 1 с — с запасом на тики и опрос.
    pub window_close_grace_ms: u64,
}

impl Default for AbExperimentConfig {
    fn default() -> Self {
        Self {
            window_ms: 60_000,
            pairs: 3,
            warmup_ms: 10_000,
            window_close_grace_ms: 1_000,
        }
    }
}

/// Десятиминутный прогон видоискателя.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EnduranceConfig {
    /// Длина прогона, мс. 10 минут — план сессии 07 («10 минут с настоящим ISP»).
    pub duration_ms: u64,
    /// Шаг снимков, мс. Минута — чтобы увидеть начало троттлинга, не раздувая отчёт.
    pub snapshot_interval_ms: u64,
}

impl Default for EnduranceConfig {
    fn default() -> Self {
        Self {
            duration_ms: 600_000,
            snapshot_interval_ms: 60_000,
        }
    }
}

/// Секция `camera.scenarios.power`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PowerMeasurementConfig {
    /// Период опроса батареи, мс. 250 мс: `CURRENT_NOW` у Qualcomm обновляется раз в сотни
    /// миллисекунд; чаще — повторы одного значения, реже — мало показаний в окне.
    pub sampling_period_ms: u64,
    /// Сколько непрочитанных показаний держит кольцо источника. 512 при 250 мс — больше двух минут:
    /// сценарий забирает показания каждый тик (50 мс), запас — на редкие задержки маршрутизатора.
    pub ring_capacity: usize,
    /// В каких единицах телефон отдаёт `CURRENT_NOW`. По умолчанию `milliamps`: так делает OnePlus 11
    /// (OxygenOS 16) вопреки документации Android (мкА) — факт 07, сверен с `dumpsys battery` Oplus
    /// («Battery current : 509» при ≈ 490 у `CURRENT_NOW`). Неверная единица не портит таблицу молча:
    /// окно отвергнет проверка правдоподобия ниже (`UnitSuspect`).
    pub current_now_unit: CurrentNowUnit,
    /// Как установить знак тока разряда.
    pub discharge_sign: DischargeSignSetting,
    /// Сколько годных ненулевых показаний нужно, чтобы определить знак. 20 — пять секунд опроса.
    pub sign_detection_min_samples: usize,
    /// Какая доля годных ненулевых показаний окна должна иметь один знак, чтобы он считался знаком
    /// разряда, в `(0,5; 1]`. 0,95: одиночный шум около нуля (1 показание из 20+) не срывает
    /// замер, а разделение без явного большинства — отказ, не догадка.
    pub sign_agreement_fraction: f64,
    /// Нижняя граница правдоподобного модуля среднего тока окна, мА. 10 мА — заведомо меньше
    /// потребления телефона с включённым экраном и камерой (сотни мА); ниже — почти наверняка
    /// ядро отдало `CURRENT_NOW` в мА вместо мкА (встречалось у OnePlus/Oppo).
    pub plausible_current_min_ma: u64,
    /// Верхняя граница правдоподобного модуля среднего тока окна, мА. 10 А — больше пикового
    /// тока батареи телефона; выше — единицы или мусор.
    pub plausible_current_max_ma: u64,
    /// Доля пропущенных показаний в окне, выше которой окно отвергается. 0,2: дыры от редких
    /// сбоев чтения допустимы, но среднее по окну с пятой частью дыр уже не про это окно.
    pub max_missing_fraction: f64,
    pub ab: AbExperimentConfig,
    pub endurance: EnduranceConfig,
}

impl Default for PowerMeasurementConfig {
    fn default() -> Self {
        Self {
            sampling_period_ms: 250,
            ring_capacity: 512,
            current_now_unit: CurrentNowUnit::Milliamps,
            discharge_sign: DischargeSignSetting::Detect,
            sign_detection_min_samples: 20,
            sign_agreement_fraction: 0.95,
            plausible_current_min_ma: 10,
            plausible_current_max_ma: 10_000,
            max_missing_fraction: 0.2,
            ab: AbExperimentConfig::default(),
            endurance: EnduranceConfig::default(),
        }
    }
}

/// Неверная секция `camera.scenarios.power`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PowerMeasurementConfigError {
    #[error("power.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("power.ab.pairs = {pairs}: нужно не меньше {minimum} пар (решение 45)")]
    TooFewPairs { pairs: u32, minimum: u32 },
    #[error("power.ab.warmup_ms ({warmup_ms}) должно быть меньше window_ms ({window_ms}): иначе мерить нечего")]
    WarmupNotShorterThanWindow { warmup_ms: u64, window_ms: u64 },
    #[error("power.sampling_period_ms ({period_ms}) должно быть меньше измеряемой части окна ({measured_ms} мс)")]
    PeriodTooLong { period_ms: u64, measured_ms: u64 },
    #[error("power.endurance.snapshot_interval_ms ({interval_ms}) больше duration_ms ({duration_ms})")]
    IntervalLongerThanRun { interval_ms: u64, duration_ms: u64 },
    #[error("power.max_missing_fraction = {value}: доля должна быть конечной, от 0 до 1 (не включая 1)")]
    MissingFractionOutOfRange { value: f64 },
    #[error("power.sign_agreement_fraction = {value}: доля должна лежать в (0,5; 1] — ничья знак не задаёт")]
    SignAgreementOutOfRange { value: f64 },
    #[error("power.plausible_current_min_ma ({min_ma}) и plausible_current_max_ma ({max_ma}): нужно 0 < min < max")]
    PlausibleCurrentRangeInvalid { min_ma: u64, max_ma: u64 },
}

/// Минимум пар по решению 45.
pub const MIN_AB_PAIRS: u32 = 3;

impl PowerMeasurementConfig {
    pub fn validate(&self) -> Result<(), PowerMeasurementConfigError> {
        for (field, value) in [
            ("sampling_period_ms", self.sampling_period_ms),
            ("ab.window_ms", self.ab.window_ms),
            ("endurance.duration_ms", self.endurance.duration_ms),
            ("endurance.snapshot_interval_ms", self.endurance.snapshot_interval_ms),
        ] {
            if value == 0 {
                return Err(PowerMeasurementConfigError::Zero { field });
            }
        }
        for (field, value) in [
            ("ring_capacity", self.ring_capacity),
            ("sign_detection_min_samples", self.sign_detection_min_samples),
        ] {
            if value == 0 {
                return Err(PowerMeasurementConfigError::Zero { field });
            }
        }
        if self.ab.pairs < MIN_AB_PAIRS {
            return Err(PowerMeasurementConfigError::TooFewPairs {
                pairs: self.ab.pairs,
                minimum: MIN_AB_PAIRS,
            });
        }
        if self.ab.warmup_ms >= self.ab.window_ms {
            return Err(PowerMeasurementConfigError::WarmupNotShorterThanWindow {
                warmup_ms: self.ab.warmup_ms,
                window_ms: self.ab.window_ms,
            });
        }
        let measured_ms = self.ab.window_ms - self.ab.warmup_ms;
        if self.sampling_period_ms >= measured_ms {
            return Err(PowerMeasurementConfigError::PeriodTooLong {
                period_ms: self.sampling_period_ms,
                measured_ms,
            });
        }
        if self.endurance.snapshot_interval_ms > self.endurance.duration_ms {
            return Err(PowerMeasurementConfigError::IntervalLongerThanRun {
                interval_ms: self.endurance.snapshot_interval_ms,
                duration_ms: self.endurance.duration_ms,
            });
        }
        // Через сравнения: NaN не проходит ни одно и отвергается.
        if !(self.max_missing_fraction >= 0.0 && self.max_missing_fraction < 1.0) {
            return Err(PowerMeasurementConfigError::MissingFractionOutOfRange {
                value: self.max_missing_fraction,
            });
        }
        self.sign_agreement()?;
        self.plausible_current_range()?;
        Ok(())
    }

    /// Порог согласия знака в типе контракта.
    fn sign_agreement(&self) -> Result<SignAgreement, PowerMeasurementConfigError> {
        SignAgreement::new(self.sign_agreement_fraction).ok_or(PowerMeasurementConfigError::SignAgreementOutOfRange {
            value: self.sign_agreement_fraction,
        })
    }

    /// Правдоподобный диапазон тока в типе контракта.
    fn plausible_current_range(&self) -> Result<PlausibleCurrentRange, PowerMeasurementConfigError> {
        PlausibleCurrentRange::from_milliamps(self.plausible_current_min_ma, self.plausible_current_max_ma).ok_or(
            PowerMeasurementConfigError::PlausibleCurrentRangeInvalid {
                min_ma: self.plausible_current_min_ma,
                max_ma: self.plausible_current_max_ma,
            },
        )
    }

    /// Проверенные настройки в типах замера (длительности, ненулевые числа, политика знака).
    pub fn settings(&self) -> Result<PowerSettings, PowerMeasurementConfigError> {
        self.validate()?;
        let zero = |field| PowerMeasurementConfigError::Zero { field };
        let period = PowerSamplingPeriod::new(Duration::from_millis(self.sampling_period_ms))
            .ok_or_else(|| zero("sampling_period_ms"))?;
        let ring_capacity = NonZeroUsize::new(self.ring_capacity).ok_or_else(|| zero("ring_capacity"))?;
        let sign = match self.discharge_sign.fixed() {
            Some(sign) => SignPolicy::Fixed(sign),
            None => SignPolicy::Detect {
                rule: SignDetectionRule {
                    required: NonZeroUsize::new(self.sign_detection_min_samples)
                        .ok_or_else(|| zero("sign_detection_min_samples"))?,
                    agreement: self.sign_agreement()?,
                },
            },
        };
        Ok(PowerSettings {
            sampling: PowerSamplingRequest {
                period,
                ring_capacity,
                current_now_unit: self.current_now_unit,
            },
            sign,
            current_range: self.plausible_current_range()?,
            max_missing_fraction: self.max_missing_fraction,
            ab: AbSettings {
                window: Duration::from_millis(self.ab.window_ms),
                warmup: Duration::from_millis(self.ab.warmup_ms),
                pairs: self.ab.pairs,
                close_grace: Duration::from_millis(self.ab.window_close_grace_ms),
            },
            endurance: EnduranceSettings {
                duration: Duration::from_millis(self.endurance.duration_ms),
                snapshot_interval: Duration::from_millis(self.endurance.snapshot_interval_ms),
            },
        })
    }
}

/// Как получить знак тока разряда.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignPolicy {
    /// Задан конфигом.
    Fixed(DischargeSign),
    /// Определить по годным показаниям (сколько нужно и какая доля должна согласиться — в правиле).
    Detect { rule: SignDetectionRule },
}

/// Проверенные настройки A/B-замера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbSettings {
    pub window: Duration,
    pub warmup: Duration,
    pub pairs: u32,
    pub close_grace: Duration,
}

/// Проверенные настройки длинного прогона.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnduranceSettings {
    pub duration: Duration,
    pub snapshot_interval: Duration,
}

/// Проверенные настройки замера целиком (строятся только через [`PowerMeasurementConfig::settings`]
/// или в тестах).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerSettings {
    /// Запрос подписки для `PowerSource::start_sampling`.
    pub sampling: PowerSamplingRequest,
    pub sign: SignPolicy,
    /// Правдоподобный модуль среднего тока окна: вне его окно отвергается как `UnitSuspect`.
    pub current_range: PlausibleCurrentRange,
    /// Доля пропущенных показаний окна, выше которой окно отвергается (0 ≤ доля < 1).
    pub max_missing_fraction: f64,
    pub ab: AbSettings,
    pub endurance: EnduranceSettings,
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
