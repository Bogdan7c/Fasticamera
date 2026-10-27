//! Синтетические показания батареи для тестов замера. Только для тестов.

use std::num::NonZeroUsize;
use std::time::Duration;

use shell_contract::clock::MonotonicNanos;
use shell_contract::power::{
    BatteryCurrent, BatterySample, ChargingState, CurrentNowUnit, DeciCelsius, DischargeSign, MicroAmpHours,
    PlausibleCurrentRange, PowerPlug, PowerSamplingPeriod, PowerSamplingRequest, SignAgreement, SignDetectionRule,
};

use super::config::{AbSettings, EnduranceSettings, PowerSettings, SignPolicy};

/// Период опроса в тестах, мс.
pub const PERIOD_MS: i64 = 250;
/// Наносекунд в миллисекунде.
pub const NANOS_PER_MS: i64 = 1_000_000;

/// Момент `ms` миллисекунд от нуля шкалы.
pub fn at_ms(ms: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(ms * NANOS_PER_MS)
}

/// Показание на батарее с током разряда `discharge_ma` мА при соглашении Android (разряд
/// отрицательный: сырое значение = −ток).
pub fn on_battery(ms: i64, discharge_ma: i64) -> BatterySample {
    BatterySample {
        at: at_ms(ms),
        current: Some(BatteryCurrent::from_raw_microamps(-discharge_ma * 1_000)),
        charge_counter: None,
        charging: ChargingState::Discharging,
        plug: PowerPlug::Unplugged,
        temperature: None,
        level_percent: Some(80),
        voltage: None,
    }
}

/// То же, но на зарядке по USB.
pub fn charging(ms: i64, discharge_ma: i64) -> BatterySample {
    BatterySample {
        charging: ChargingState::Charging,
        plug: PowerPlug::Usb,
        ..on_battery(ms, discharge_ma)
    }
}

/// Показания каждые `PERIOD_MS` на `[from_ms, to_ms)`; ток задаёт `current_at(ms)`.
pub fn series(from_ms: i64, to_ms: i64, current_at: impl Fn(i64) -> i64) -> Vec<BatterySample> {
    (from_ms..to_ms)
        .step_by(usize::try_from(PERIOD_MS).expect("период"))
        .map(|ms| on_battery(ms, current_at(ms)))
        .collect()
}

pub fn period() -> PowerSamplingPeriod {
    PowerSamplingPeriod::new(Duration::from_millis(PERIOD_MS as u64)).expect("период не ноль")
}

/// Настройки: окно `window_ms`, разогрев `warmup_ms`, `pairs` пар, знак — автоопределение по 4 показаниям.
pub fn settings(window_ms: u64, warmup_ms: u64, pairs: u32) -> PowerSettings {
    PowerSettings {
        sampling: PowerSamplingRequest {
            period: period(),
            ring_capacity: NonZeroUsize::new(64).expect("не ноль"),
            current_now_unit: CurrentNowUnit::Microamps,
        },
        sign: SignPolicy::Detect {
            rule: SignDetectionRule {
                required: NonZeroUsize::new(4).expect("не ноль"),
                // Строгое согласие: тесты противоречий знака проверяют именно отказ.
                agreement: SignAgreement::new(1.0).expect("доля"),
            },
        },
        current_range: phone_current_range(),
        max_missing_fraction: 0.2,
        ab: AbSettings {
            window: Duration::from_millis(window_ms),
            warmup: Duration::from_millis(warmup_ms),
            pairs,
            close_grace: Duration::from_millis(500),
        },
        endurance: EnduranceSettings {
            duration: Duration::from_millis(10 * window_ms),
            snapshot_interval: Duration::from_millis(window_ms),
        },
    }
}

/// Настройки с явно заданным знаком.
pub fn settings_with_sign(window_ms: u64, warmup_ms: u64, pairs: u32, sign: DischargeSign) -> PowerSettings {
    PowerSettings {
        sign: SignPolicy::Fixed(sign),
        ..settings(window_ms, warmup_ms, pairs)
    }
}

/// Добавляет счётчик заряда и температуру к показанию.
pub fn with_counter(mut sample: BatterySample, microamp_hours: i64, deci_celsius: i32) -> BatterySample {
    sample.charge_counter = Some(MicroAmpHours::from_microamp_hours(microamp_hours));
    sample.temperature = Some(DeciCelsius::from_deci_celsius(deci_celsius));
    sample
}

/// Правдоподобный ток телефона, как по умолчанию в конфиге: [10 мА, 10 А].
pub fn phone_current_range() -> PlausibleCurrentRange {
    PlausibleCurrentRange::from_milliamps(10, 10_000).expect("диапазон")
}
