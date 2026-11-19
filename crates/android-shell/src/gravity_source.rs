//! Гравитация для блика стекла: настройки, выбор датчика и фильтр (нейтральная часть, тестируется на ПК).
//!
//! Блику нужен непрерывный вектор наклона телефона (`shell_contract::gravity`). Путь получения:
//! 1. **Датчик гравитации платформы** (`ASENSOR_TYPE_GRAVITY`) — вектор уже очищен от ускорения руки
//!    (ядро Android сливает акселерометр и гироскоп). Берём его, если он есть.
//! 2. **Откат:** акселерометр с низкочастотным фильтром ([`GravityIntake::filtered`]) — тот же приём,
//!    что в документации Android по датчикам движения.
//!
//! Android-часть (поток датчика, `ASensorEvent`) в `android::services`; здесь только то, что не
//! зависит от NDK: перевод показания в [`GravitySample`], фильтр и правило отката.

use std::num::NonZeroU32;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shell_contract::clock::BoottimeNanos;
use shell_contract::gravity::{GravitySample, GravitySource, GravityVector};
use shell_contract::motion::{SensorRate, SensorSourceError};

/// Настройки гравитации. Приходят в составе `ShellConfig` (поле `gravity`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GravityConfig {
    /// Частота датчика, Гц. По умолчанию 50 `[estimate]`: наклон руки меняется на единицах герц
    /// (до ~10), 50 Гц даёт пятикратный запас по Найквисту, кадр UI (60–120 Гц) всё равно сглаживает
    /// по времени, а частота ниже сотни Гц почти не добавляет расход батареи. Подгонка, не Apple.
    pub rate_hz: u32,
    /// Постоянная времени фильтра отката на акселерометр, мс (0 — без фильтра). По умолчанию 200
    /// `[estimate]`: частота среза ≈ 0,8 Гц отделяет медленный наклон от тряски руки, а блик
    /// отстаёт от наклона не заметнее, чем у сглаживания потребителя. Подгонка, не Apple.
    /// На датчик гравитации платформы не влияет.
    pub accelerometer_time_constant_ms: u64,
}

impl Default for GravityConfig {
    fn default() -> Self {
        Self {
            rate_hz: 50,
            accelerometer_time_constant_ms: 200,
        }
    }
}

/// Неверные настройки гравитации.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GravityConfigError {
    #[error("gravity.rate_hz должно быть больше нуля")]
    ZeroRate,
}

/// Проверенные настройки гравитации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GravitySettings {
    pub rate: SensorRate,
    pub accelerometer_time_constant: Duration,
}

impl GravityConfig {
    pub fn validate(&self) -> Result<GravitySettings, GravityConfigError> {
        let hertz = NonZeroU32::new(self.rate_hz).ok_or(GravityConfigError::ZeroRate)?;
        Ok(GravitySettings {
            rate: SensorRate::new(hertz),
            accelerometer_time_constant: Duration::from_millis(self.accelerometer_time_constant_ms),
        })
    }
}

/// Что стало с очередным показанием датчика.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GravityIntakeOutcome {
    /// Показание принято: положить в ящик гравитации.
    Publish(GravitySample),
    /// Отброшено: компонента не число (сбой датчика). Старое значение остаётся в ящике.
    RejectedNonFinite,
    /// Отброшено: метка не позже предыдущей (фильтру нужно строго растущее время).
    RejectedOutOfOrder,
}

/// Превращает сырые показания датчика в [`GravitySample`] выбранным путём.
#[derive(Debug, Clone)]
pub struct GravityIntake {
    source: GravitySource,
    /// Состояние фильтра; `None` у датчика гравитации платформы (фильтр не нужен).
    low_pass: Option<LowPassState>,
    rejected_samples: u64,
}

#[derive(Debug, Clone)]
struct LowPassState {
    time_constant: Duration,
    /// Последнее выданное показание; `None` до первого.
    last: Option<GravitySample>,
}

impl GravityIntake {
    /// Показания готового датчика гравитации: проходят как есть (кроме не-чисел).
    pub fn dedicated() -> Self {
        Self {
            source: GravitySource::DedicatedSensor,
            low_pass: None,
            rejected_samples: 0,
        }
    }

    /// Показания акселерометра с экспоненциальным фильтром `α = Δt / (τ + Δt)` — вес нового
    /// показания (формула из руководства Android по датчикам движения, `τ` — постоянная времени).
    /// Первое показание принимается как есть, дальше вектор подтягивается к свежему.
    pub fn filtered(time_constant: Duration) -> Self {
        Self {
            source: GravitySource::FilteredAccelerometer,
            low_pass: Some(LowPassState {
                time_constant,
                last: None,
            }),
            rejected_samples: 0,
        }
    }

    pub fn source(&self) -> GravitySource {
        self.source
    }

    /// Сколько показаний отброшено с начала (не-числа и непорядок времени).
    pub fn rejected_samples(&self) -> u64 {
        self.rejected_samples
    }

    /// Принимает показание `components` (x, y, z, м/с²) с меткой `time` (`BOOTTIME`).
    pub fn accept(&mut self, time: BoottimeNanos, components: [f32; 3]) -> GravityIntakeOutcome {
        let [x, y, z] = components;
        if !(x.is_finite() && y.is_finite() && z.is_finite()) {
            self.rejected_samples += 1;
            return GravityIntakeOutcome::RejectedNonFinite;
        }
        let raw = GravitySample {
            time,
            vector: GravityVector { x, y, z },
        };
        let Some(low_pass) = &mut self.low_pass else {
            return GravityIntakeOutcome::Publish(raw);
        };
        match low_pass.smooth(raw) {
            Some(smoothed) => GravityIntakeOutcome::Publish(smoothed),
            None => {
                self.rejected_samples += 1;
                GravityIntakeOutcome::RejectedOutOfOrder
            }
        }
    }
}

impl LowPassState {
    /// `None`, если метка не позже последней.
    fn smooth(&mut self, raw: GravitySample) -> Option<GravitySample> {
        let Some(previous) = self.last else {
            self.last = Some(raw);
            return Some(raw);
        };
        let delta_ns = raw.time.as_nanos().checked_sub(previous.time.as_nanos())?;
        if delta_ns <= 0 {
            return None;
        }
        let delta_seconds = delta_ns as f64 * 1e-9;
        let alpha = delta_seconds / (self.time_constant.as_secs_f64() + delta_seconds);
        // alpha в (0, 1]: при τ = 0 — равно 1 (без фильтра), при очень большом Δt — почти 1.
        let blend = |old: f32, new: f32| (f64::from(old) + alpha * (f64::from(new) - f64::from(old))) as f32;
        let smoothed = GravitySample {
            time: raw.time,
            vector: GravityVector {
                x: blend(previous.vector.x, raw.vector.x),
                y: blend(previous.vector.y, raw.vector.y),
                z: blend(previous.vector.z, raw.vector.z),
            },
        };
        self.last = Some(smoothed);
        Some(smoothed)
    }
}

/// Пробует датчик гравитации платформы; если его на телефоне нет — запускает откат на акселерометр.
///
/// Откат только при [`SensorSourceError::SensorAbsent`]: любая другая ошибка (сбой регистрации,
/// датчик уже занят) — настоящая проблема, и подменять её другим путём нельзя, иначе она спряталась бы.
/// Возвращает запущенное и путь, по которому оно получено.
pub fn start_with_fallback<Started>(
    start_dedicated: impl FnOnce() -> Result<Started, SensorSourceError>,
    start_accelerometer: impl FnOnce() -> Result<Started, SensorSourceError>,
) -> Result<(Started, GravitySource), SensorSourceError> {
    match start_dedicated() {
        Ok(started) => Ok((started, GravitySource::DedicatedSensor)),
        Err(SensorSourceError::SensorAbsent { .. }) => {
            start_accelerometer().map(|started| (started, GravitySource::FilteredAccelerometer))
        }
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(milliseconds: i64) -> BoottimeNanos {
        BoottimeNanos::from_nanos(milliseconds * 1_000_000)
    }

    fn published(outcome: GravityIntakeOutcome) -> GravitySample {
        match outcome {
            GravityIntakeOutcome::Publish(sample) => sample,
            other => panic!("ожидали Publish, получили {other:?}"),
        }
    }

    #[test]
    fn dedicated_passes_values_through_and_rejects_non_finite() {
        let mut intake = GravityIntake::dedicated();
        let sample = published(intake.accept(at(1), [0.5, 9.7, 1.0]));
        assert_eq!(sample.vector, GravityVector { x: 0.5, y: 9.7, z: 1.0 });
        assert_eq!(sample.time, at(1));
        assert_eq!(
            intake.accept(at(2), [f32::NAN, 0.0, 0.0]),
            GravityIntakeOutcome::RejectedNonFinite
        );
        assert_eq!(intake.rejected_samples(), 1);
        assert_eq!(intake.source(), GravitySource::DedicatedSensor);
        // У датчика платформы порядок времени не проверяем: ящик хранит просто последнее.
        assert!(matches!(
            intake.accept(at(1), [0.0, 9.8, 0.0]),
            GravityIntakeOutcome::Publish(_)
        ));
    }

    #[test]
    fn filter_starts_from_first_sample_then_converges_to_gravity() {
        let mut intake = GravityIntake::filtered(Duration::from_millis(200));
        // Телефон лежал экраном вверх, затем встал в портрет: вектор (0,0,g) -> (0,g,0).
        let first = published(intake.accept(at(0), [0.0, 0.0, 9.81]));
        assert_eq!(
            first.vector,
            GravityVector {
                x: 0.0,
                y: 0.0,
                z: 9.81
            }
        );
        // Один шаг 20 мс при τ = 200 мс: α = 20 / 220 ≈ 0,0909.
        let step = published(intake.accept(at(20), [0.0, 9.81, 0.0]));
        assert!(
            (step.vector.y - 9.81 * (20.0 / 220.0)).abs() < 1e-4,
            "y = {}",
            step.vector.y
        );
        assert!((step.vector.z - 9.81 * (1.0 - 20.0 / 220.0)).abs() < 1e-4);
        // За две секунды (≈ 10 τ) вектор доезжает до нового положения.
        let mut last = step;
        for tick in 2..=100 {
            last = published(intake.accept(at(tick * 20), [0.0, 9.81, 0.0]));
        }
        assert!((last.vector.y - 9.81).abs() < 0.01 && last.vector.z.abs() < 0.01);
    }

    #[test]
    fn filter_suppresses_a_short_hand_shake() {
        let mut intake = GravityIntake::filtered(Duration::from_millis(200));
        published(intake.accept(at(0), [0.0, 9.81, 0.0]));
        // Один выброс +5 м/с² по x на 20 мс не должен заметно сдвинуть вектор.
        let spike = published(intake.accept(at(20), [5.0, 9.81, 0.0]));
        assert!(spike.vector.x > 0.0 && spike.vector.x < 0.5, "x = {}", spike.vector.x);
    }

    #[test]
    fn zero_time_constant_means_no_smoothing() {
        let mut intake = GravityIntake::filtered(Duration::ZERO);
        published(intake.accept(at(0), [0.0, 0.0, 9.81]));
        let sample = published(intake.accept(at(20), [1.0, 2.0, 3.0]));
        assert_eq!(sample.vector, GravityVector { x: 1.0, y: 2.0, z: 3.0 });
    }

    #[test]
    fn filter_rejects_out_of_order_and_keeps_state() {
        let mut intake = GravityIntake::filtered(Duration::from_millis(200));
        published(intake.accept(at(10), [0.0, 9.81, 0.0]));
        assert_eq!(
            intake.accept(at(10), [9.0, 0.0, 0.0]),
            GravityIntakeOutcome::RejectedOutOfOrder
        );
        assert_eq!(
            intake.accept(at(5), [9.0, 0.0, 0.0]),
            GravityIntakeOutcome::RejectedOutOfOrder
        );
        assert_eq!(intake.rejected_samples(), 2);
        // Отброшенные показания фильтр не испортили: следующий шаг считается от (0, g, 0).
        let next = published(intake.accept(at(30), [0.0, 9.81, 0.0]));
        assert_eq!(
            next.vector,
            GravityVector {
                x: 0.0,
                y: 9.81,
                z: 0.0
            }
        );
        assert_eq!(intake.source(), GravitySource::FilteredAccelerometer);
    }

    #[test]
    fn fallback_only_when_dedicated_sensor_is_absent() {
        let absent = || Err(SensorSourceError::SensorAbsent { sensor: "gravity" });
        let (started, source) = start_with_fallback(|| Ok("dedicated"), || Ok("accelerometer")).unwrap();
        assert_eq!((started, source), ("dedicated", GravitySource::DedicatedSensor));
        let (started, source) = start_with_fallback(absent, || Ok("accelerometer")).unwrap();
        assert_eq!(
            (started, source),
            ("accelerometer", GravitySource::FilteredAccelerometer)
        );
        // Сбой платформы не маскируется откатом, и запасной путь даже не запускается.
        let broken = || {
            Err::<&str, _>(SensorSourceError::PlatformFailure {
                sensor: "gravity",
                reason: "регистрация отклонена".to_owned(),
            })
        };
        let error = start_with_fallback(broken, || panic!("откат не должен запускаться")).unwrap_err();
        assert!(matches!(error, SensorSourceError::PlatformFailure { .. }));
        // Если нет и акселерометра — ошибка отката доходит до вызывающего.
        let error = start_with_fallback(absent, absent).unwrap_err();
        assert_eq!(error, SensorSourceError::SensorAbsent { sensor: "gravity" });
    }

    #[test]
    fn config_defaults_are_valid_and_zero_rate_is_rejected() {
        let settings = GravityConfig::default().validate().unwrap();
        assert_eq!(settings.rate.hertz(), 50);
        assert_eq!(settings.accelerometer_time_constant, Duration::from_millis(200));
        let zero = GravityConfig {
            rate_hz: 0,
            ..GravityConfig::default()
        };
        assert_eq!(zero.validate(), Err(GravityConfigError::ZeroRate));
        assert!(serde_json::from_str::<GravityConfig>(r#"{"rate": 10}"#).is_err());
    }
}
