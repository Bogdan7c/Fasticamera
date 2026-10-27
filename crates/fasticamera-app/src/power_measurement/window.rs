//! Одно окно замера тока: показания батареи за интервал → средний ток разряда и сопутствующие числа.
//!
//! Окно `[начало, конец)` делится на разогрев `[начало, начало + разогрев)` (показания только
//! проверяются на внешнее питание, в среднее не идут) и измеряемую часть `[начало + разогрев, конец)`.
//! Правила:
//! * хоть одно показание окна (включая разогрев) на внешнем питании или с неизвестным источником
//!   питания — окно отвергается целиком (решение 45: на зарядке ток батареи — не ток телефона);
//! * среднее — арифметическое по показаниям с током: опрос равномерный, а дыры не маскируются
//!   интерполяцией, а считаются отдельно (`missing_samples`) и при доле выше порога отвергают окно;
//! * знак тока не угадывается: [`PowerWindowAccumulator::finish`] получает его явно;
//! * единицы проверяются: модуль среднего сырого тока вне правдоподобного диапазона
//!   ([`PlausibleCurrentRange`]) — отказ `UnitSuspect`, а не число, неверное в 1000 раз.
//!
//! Модуль чистый: ни часов, ни платформы — только показания, которые ему передали.

use std::time::Duration;

use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use shell_contract::power::{
    BatterySample, ChargingState, CurrentPlausibility, DeciCelsius, DischargeSign, MeasurementValidity,
    PlausibleCurrentRange, PowerPlug, PowerSamplingPeriod,
};

/// Микроампер в миллиампере и микроампер-часов в миллиампер-часе.
const MICRO_PER_MILLI: f64 = 1_000.0;
/// Наносекунд в миллисекунде.
const NANOS_PER_MILLI: i64 = 1_000_000;

/// Границы окна на шкале `CLOCK_MONOTONIC` (той же, что у `BatterySample::at`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSpan {
    start: MonotonicNanos,
    measure_from: MonotonicNanos,
    end: MonotonicNanos,
}

impl WindowSpan {
    /// Окно длиной `length` с начала `start`, из которого первые `warmup` выбрасываются. Разогрев
    /// длиннее окна обрезается до окна (тогда мерить нечего, и окно отвергнется как пустое).
    pub fn new(start: MonotonicNanos, warmup: Duration, length: Duration) -> Self {
        let end = offset(start, length);
        let measure_from = offset(start, warmup.min(length));
        Self {
            start,
            measure_from,
            end,
        }
    }

    pub fn start(&self) -> MonotonicNanos {
        self.start
    }

    pub fn end(&self) -> MonotonicNanos {
        self.end
    }

    /// Лежит ли момент в окне (`начало <= at < конец`).
    pub fn contains(&self, at: MonotonicNanos) -> bool {
        self.start <= at && at < self.end
    }

    /// Лежит ли момент в разогреве.
    pub fn is_warmup(&self, at: MonotonicNanos) -> bool {
        self.start <= at && at < self.measure_from
    }

    fn measured_nanos(&self) -> i64 {
        self.end.as_nanos().saturating_sub(self.measure_from.as_nanos())
    }
}

/// Момент `from + by` с насыщением (`Duration` в наносекундах больше `i64` не бывает на практике).
pub fn offset(from: MonotonicNanos, by: Duration) -> MonotonicNanos {
    let nanos = i64::try_from(by.as_nanos()).unwrap_or(i64::MAX);
    MonotonicNanos::from_nanos(from.as_nanos().saturating_add(nanos))
}

/// Почему окно не годится для замера.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowRejection {
    /// Телефон на внешнем питании: ток батареи — разность зарядки и потребления.
    ExternalPower { plug: PowerPlug, charging: ChargingState },
    /// Платформа не сказала, подключено ли питание.
    PowerStateUnknown { charging: ChargingState },
    /// В измеряемой части нет ни одного показания с током.
    NoCurrentReadings,
    /// Пропущенных показаний больше допустимой доли.
    TooManyMissingSamples { missing: u64, expected: u64 },
    /// Модуль среднего тока вне правдоподобного диапазона: вероятно, ядро отдаёт `CURRENT_NOW`
    /// не в микроамперах (см. `PlausibleCurrentRange`). Число не выдаётся.
    UnitSuspect {
        /// Модуль среднего сырого тока окна, как если бы он был в мкА, — в мА.
        mean_abs_ma: f64,
        plausibility: CurrentPlausibility,
    },
}

/// Что стало с показанием, переданным окну.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowPush {
    /// Попало в разогрев: проверено на питание, в среднее не идёт.
    WarmUp,
    /// Попало в измеряемую часть.
    Measured,
    /// Не из этого окна — не принято (решать, куда его деть, должен вызывающий).
    OutsideWindow,
    /// Окно отвергнуто (этим или более ранним показанием).
    Rejected(WindowRejection),
}

/// Итог годного окна.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WindowMeasurement {
    /// Средний ток разряда по измеряемой части, мА (положительный — батарея отдаёт энергию).
    pub mean_discharge_ma: f64,
    /// Показаний в измеряемой части (с током и без).
    pub samples: u64,
    /// Из них без тока (свойство не прочиталось) — в среднее не вошли.
    pub samples_without_current: u64,
    /// Показаний в разогреве.
    pub warmup_samples: u64,
    /// Сколько показаний ожидалось в измеряемой части при заданном периоде.
    pub expected_samples: u64,
    /// `expected_samples − samples`, не меньше нуля.
    pub missing_samples: u64,
    /// Самая длинная пауза между соседними показаниями (включая края измеряемой части), мс.
    pub longest_gap_ms: u64,
    /// Израсходованный заряд по `CHARGE_COUNTER` между первым и последним показанием с ним, мА·ч.
    /// `None` — счётчика нет (вендор не отдаёт) или он прочитался меньше двух раз.
    pub charge_used_mah: Option<f64>,
    /// Температура батареи в начале и в конце измеряемой части.
    pub temperature_start: Option<DeciCelsius>,
    pub temperature_end: Option<DeciCelsius>,
}

/// Накопитель показаний одного окна.
#[derive(Debug, Clone)]
pub struct PowerWindowAccumulator {
    span: WindowSpan,
    period: PowerSamplingPeriod,
    measured: Vec<BatterySample>,
    warmup_samples: u64,
    rejection: Option<WindowRejection>,
}

impl PowerWindowAccumulator {
    pub fn new(span: WindowSpan, period: PowerSamplingPeriod) -> Self {
        Self {
            span,
            period,
            measured: Vec::new(),
            warmup_samples: 0,
            rejection: None,
        }
    }

    pub fn span(&self) -> WindowSpan {
        self.span
    }

    /// Принимает показание. Первое показание не на батарее отвергает окно навсегда.
    pub fn push(&mut self, sample: BatterySample) -> WindowPush {
        if !self.span.contains(sample.at) {
            return WindowPush::OutsideWindow;
        }
        if let Some(rejection) = self.rejection {
            return WindowPush::Rejected(rejection);
        }
        if let Some(rejection) = power_rejection(&sample) {
            self.rejection = Some(rejection);
            return WindowPush::Rejected(rejection);
        }
        if self.span.is_warmup(sample.at) {
            self.warmup_samples += 1;
            WindowPush::WarmUp
        } else {
            self.measured.push(sample);
            WindowPush::Measured
        }
    }

    /// Отвергнуто ли окно уже сейчас (до конца).
    pub fn rejection(&self) -> Option<WindowRejection> {
        self.rejection
    }

    /// Показания измеряемой части (для определения знака тока по этому окну).
    pub fn measured_samples(&self) -> &[BatterySample] {
        &self.measured
    }

    /// Годно ли окно без учёта знака тока: не отвергнуто по питанию и дыр не больше доли
    /// `max_missing_fraction`. Проверяется ДО определения знака: пустое окно — это «дыры», а не
    /// «знак не установлен».
    pub fn check_complete(&self, max_missing_fraction: f64) -> Result<(), WindowRejection> {
        if let Some(rejection) = self.rejection {
            return Err(rejection);
        }
        let expected_samples = self.expected_samples();
        let missing_samples = expected_samples.saturating_sub(count(self.measured.len()));
        if expected_samples > 0 && ratio(missing_samples, expected_samples) > max_missing_fraction {
            return Err(WindowRejection::TooManyMissingSamples {
                missing: missing_samples,
                expected: expected_samples,
            });
        }
        Ok(())
    }

    /// Итог окна при известном знаке тока, пороге пропусков (`0 ≤ max_missing_fraction < 1`) и
    /// правдоподобном диапазоне тока `current_range`.
    pub fn finish(
        &self,
        sign: DischargeSign,
        max_missing_fraction: f64,
        current_range: PlausibleCurrentRange,
    ) -> Result<WindowMeasurement, WindowRejection> {
        self.check_complete(max_missing_fraction)?;
        self.check_current_units(current_range)?;
        let expected_samples = self.expected_samples();
        let samples = count(self.measured.len());
        let missing_samples = expected_samples.saturating_sub(samples);
        let mean_discharge_ma = self.mean_discharge_ma(sign).ok_or(WindowRejection::NoCurrentReadings)?;
        let with_current = self.measured.iter().filter(|sample| sample.current.is_some()).count();
        Ok(WindowMeasurement {
            mean_discharge_ma,
            samples,
            samples_without_current: samples.saturating_sub(count(with_current)),
            warmup_samples: self.warmup_samples,
            expected_samples,
            missing_samples,
            longest_gap_ms: self.longest_gap_ms(),
            charge_used_mah: self.charge_used_mah(),
            temperature_start: self.measured.iter().find_map(|sample| sample.temperature),
            temperature_end: self.measured.iter().rev().find_map(|sample| sample.temperature),
        })
    }

    /// Модуль среднего сырого тока (от знака не зависит) обязан быть правдоподобным. Окно без
    /// тока здесь не отвергается — это `NoCurrentReadings` ниже.
    fn check_current_units(&self, current_range: PlausibleCurrentRange) -> Result<(), WindowRejection> {
        let magnitudes: Vec<u64> = self
            .measured
            .iter()
            .filter_map(|sample| sample.current)
            .map(|current| current.raw_microamps().unsigned_abs())
            .collect();
        if magnitudes.is_empty() {
            return Ok(());
        }
        // Сумма в u128: переполнение исключено при любом числе показаний.
        let sum: u128 = magnitudes.iter().map(|&microamps| u128::from(microamps)).sum();
        // Сумма тока телефона за окно ≪ 2^53 мкА, перевод в f64 точен.
        let mean_abs_microamps = sum as f64 / magnitudes.len() as f64;
        match current_range.classify(mean_abs_microamps) {
            CurrentPlausibility::Plausible => Ok(()),
            plausibility => Err(WindowRejection::UnitSuspect {
                mean_abs_ma: mean_abs_microamps / MICRO_PER_MILLI,
                plausibility,
            }),
        }
    }

    /// Сколько показаний помещается в измеряемую часть при равномерном опросе.
    fn expected_samples(&self) -> u64 {
        let period = i64::try_from(self.period.duration().as_nanos()).unwrap_or(i64::MAX);
        u64::try_from(self.span.measured_nanos() / period.max(1)).unwrap_or(0)
    }

    fn mean_discharge_ma(&self, sign: DischargeSign) -> Option<f64> {
        let currents: Vec<i64> = self
            .measured
            .iter()
            .filter_map(|sample| sample.current)
            .map(|current| current.discharge_current(sign).microamps())
            .collect();
        if currents.is_empty() {
            return None;
        }
        // Сумма в i128: переполнение i64 исключено при любом числе показаний.
        let sum: i128 = currents.iter().map(|&microamps| i128::from(microamps)).sum();
        // Сумма тока телефона за окно ≪ 2^53 мкА, перевод в f64 точен.
        Some(sum as f64 / currents.len() as f64 / MICRO_PER_MILLI)
    }

    fn charge_used_mah(&self) -> Option<f64> {
        let mut counters = self.measured.iter().filter_map(|sample| sample.charge_counter);
        let first = counters.next()?;
        let last = counters.next_back()?;
        // Разность счётчика телефона ≪ 2^53 мкА·ч, перевод в f64 точен.
        Some(last.consumed_since(first) as f64 / MICRO_PER_MILLI)
    }

    /// Самая длинная пауза: от начала измеряемой части до первого показания, между соседними и от
    /// последнего до конца окна. Окно без показаний — пауза во всю измеряемую часть.
    fn longest_gap_ms(&self) -> u64 {
        let edges = std::iter::once(self.span.measure_from.as_nanos())
            .chain(self.measured.iter().map(|sample| sample.at.as_nanos()))
            .chain(std::iter::once(self.span.end.as_nanos()));
        let mut previous: Option<i64> = None;
        let mut longest = 0_i64;
        for moment in edges {
            if let Some(before) = previous {
                longest = longest.max(moment.saturating_sub(before));
            }
            previous = Some(moment);
        }
        u64::try_from(longest / NANOS_PER_MILLI).unwrap_or(0)
    }
}

/// Отказ по источнику питания показания; `None` — телефон на батарее.
fn power_rejection(sample: &BatterySample) -> Option<WindowRejection> {
    match sample.measurement_validity() {
        MeasurementValidity::Valid => None,
        MeasurementValidity::RejectedExternalPower { plug, charging } => {
            Some(WindowRejection::ExternalPower { plug, charging })
        }
        MeasurementValidity::RejectedPowerStateUnknown { charging } => {
            Some(WindowRejection::PowerStateUnknown { charging })
        }
    }
}

/// Число элементов как u64 (на 64-битных целях без потерь).
fn count(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// Доля `part / whole` (оба числа ≪ 2^53 — окно в минуты при опросе в сотни миллисекунд).
fn ratio(part: u64, whole: u64) -> f64 {
    part as f64 / whole as f64
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
