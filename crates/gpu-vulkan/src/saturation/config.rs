//! Настройки прогона стенда насыщения и их проверка (до любых команд GPU).

use std::time::Duration;

use crate::swapchain::FramesInFlight;

/// Прогрев по умолчанию: драйвер разгоняет частоту GPU не мгновенно, первые секунды в цифру не идут.
pub const DEFAULT_WARMUP: Duration = Duration::from_secs(2);
/// Окно замера по умолчанию.
pub const DEFAULT_MEASURE: Duration = Duration::from_secs(5);
/// Шаг временной шкалы по умолчанию: одна секунда — виден и разгон, и троттлинг.
pub const DEFAULT_TIMELINE_STEP: Duration = Duration::from_secs(1);
/// Предел ожидания одного fence по умолчанию: дольше считаем GPU зависшим.
pub const DEFAULT_FENCE_TIMEOUT: Duration = Duration::from_secs(5);
/// Верхняя граница числа шагов шкалы: шкала выделяется заранее, абсурдный шаг не должен съесть память.
pub const MAX_TIMELINE_STEPS: u64 = 100_000;

/// Настройки прогона [`super::SaturationBench`].
///
/// Поля публичные, но значения проверяет [`Self::validate`] (его вызывает конструктор стенда ДО создания
/// объектов Vulkan). `frames_in_flight` — типом `FramesInFlight`: 0 слотов невозможен по построению.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaturationConfig {
    /// Сколько кадров работы одновременно стоит в очереди GPU (слотов стенда).
    pub frames_in_flight: FramesInFlight,
    /// Сколько времени крутим работу до начала замера. Может быть нулём (замер с первой секунды).
    pub warmup: Duration,
    /// Длительность окна замера (> 0).
    pub measure: Duration,
    /// Шаг временной шкалы внутри окна (> 0).
    pub timeline_step: Duration,
    /// Сколько максимум ждём один fence (> 0); дольше — ошибка `Timeout`, а не вечное зависание.
    pub fence_timeout: Duration,
}

impl Default for SaturationConfig {
    fn default() -> Self {
        Self {
            frames_in_flight: FramesInFlight::DEFAULT,
            warmup: DEFAULT_WARMUP,
            measure: DEFAULT_MEASURE,
            timeline_step: DEFAULT_TIMELINE_STEP,
            fence_timeout: DEFAULT_FENCE_TIMEOUT,
        }
    }
}

/// Что не так с настройками.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SaturationConfigError {
    /// Нулевое окно замера: делить будет не на что.
    #[error("длительность окна замера должна быть > 0")]
    ZeroMeasureWindow,
    /// Нулевой шаг шкалы.
    #[error("шаг временной шкалы должен быть > 0")]
    ZeroTimelineStep,
    /// Нулевой таймаут fence: каждое ожидание сразу считалось бы зависанием.
    #[error("таймаут ожидания fence должен быть > 0")]
    ZeroFenceTimeout,
    /// Шаг слишком мелкий для окна: шкала получилась бы длиннее [`MAX_TIMELINE_STEPS`].
    #[error("шаг шкалы даёт {steps} шагов (допустимо не больше {MAX_TIMELINE_STEPS})")]
    TooManyTimelineSteps {
        /// Сколько шагов получилось бы.
        steps: u64,
    },
}

impl SaturationConfig {
    /// Проверяет значения. Ничего не трогает на GPU.
    pub fn validate(&self) -> Result<(), SaturationConfigError> {
        if self.measure.is_zero() {
            return Err(SaturationConfigError::ZeroMeasureWindow);
        }
        if self.timeline_step.is_zero() {
            return Err(SaturationConfigError::ZeroTimelineStep);
        }
        if self.fence_timeout.is_zero() {
            return Err(SaturationConfigError::ZeroFenceTimeout);
        }
        let steps = self.timeline_step_count();
        if steps > MAX_TIMELINE_STEPS {
            return Err(SaturationConfigError::TooManyTimelineSteps { steps });
        }
        Ok(())
    }

    /// Число шагов шкалы для полного окна: окно делим на шаг с округлением вверх (последний шаг может быть короче).
    pub(crate) fn timeline_step_count(&self) -> u64 {
        // Деление в наносекундах: u128 не переполняется на любых Duration; шаг > 0 проверен в validate.
        let step_nanos = self.timeline_step.as_nanos().max(1);
        let steps = self.measure.as_nanos().div_ceil(step_nanos);
        u64::try_from(steps).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert_eq!(SaturationConfig::default().validate(), Ok(()));
    }

    #[test]
    fn invalid_values_are_reported_with_specific_errors() {
        let base = SaturationConfig::default();
        let zero_measure = SaturationConfig {
            measure: Duration::ZERO,
            ..base
        };
        let zero_step = SaturationConfig {
            timeline_step: Duration::ZERO,
            ..base
        };
        let zero_timeout = SaturationConfig {
            fence_timeout: Duration::ZERO,
            ..base
        };
        let tiny_step = SaturationConfig {
            timeline_step: Duration::from_nanos(1),
            ..base
        };
        assert_eq!(zero_measure.validate(), Err(SaturationConfigError::ZeroMeasureWindow));
        assert_eq!(zero_step.validate(), Err(SaturationConfigError::ZeroTimelineStep));
        assert_eq!(zero_timeout.validate(), Err(SaturationConfigError::ZeroFenceTimeout));
        assert!(matches!(
            tiny_step.validate(),
            Err(SaturationConfigError::TooManyTimelineSteps { .. })
        ));
    }

    #[test]
    fn step_count_rounds_up_so_partial_last_step_exists() {
        let config = SaturationConfig {
            measure: Duration::from_millis(2500),
            timeline_step: Duration::from_secs(1),
            ..SaturationConfig::default()
        };
        assert_eq!(config.timeline_step_count(), 3);
        let step_longer_than_window = SaturationConfig {
            measure: Duration::from_millis(300),
            timeline_step: Duration::from_secs(1),
            ..SaturationConfig::default()
        };
        assert_eq!(step_longer_than_window.timeline_step_count(), 1);
    }

    #[test]
    fn warmup_may_be_zero() {
        let config = SaturationConfig {
            warmup: Duration::ZERO,
            ..SaturationConfig::default()
        };
        assert_eq!(config.validate(), Ok(()));
    }
}
