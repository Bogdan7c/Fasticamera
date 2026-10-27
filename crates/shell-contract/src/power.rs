//! Питание: показания батареи для замера тока по стадиям ISP (сессия 07, решение 45).
//!
//! Зачем: вклад стадии в расход батареи меряется A/B-чередованием («со стадией» − «без неё»,
//! по 60 с, не меньше трёх пар) и десятиминутным прогоном. Источник на телефоне — Android
//! `BatteryManager` (`BATTERY_PROPERTY_CURRENT_NOW`, `BATTERY_PROPERTY_CHARGE_COUNTER`) и
//! липкое намерение `ACTION_BATTERY_CHANGED` (статус, источник питания, температура, уровень,
//! напряжение). sysfs без root закрыт (факт сессии 05).
//!
//! Раскладка:
//! * этот файл — одно показание [`BatterySample`], единицы измерения, знак тока и
//!   [`MeasurementValidity`] (на внешнем питании ток мерить бессмысленно);
//! * [`sampling`] — неблокирующий трейт [`PowerSource`], ограниченное кольцо показаний и
//!   учёт потерь и сбоев чтения;
//! * `fake` (фича `fake-power-source` или тесты) — подставной источник для тестов потребителей.
//!
//! Знак тока — ловушка. По документации Android положительный `CURRENT_NOW` — ток В батарею
//! (заряд), отрицательный — разряд. Многие производители знак инвертируют. Поэтому показание
//! хранит СЫРОЕ значение ([`BatteryCurrent::raw_microamps`]), а «ток разряда» получается только
//! с явно названным соглашением [`DischargeSign`]. Соглашение либо задаёт конфиг, либо его
//! подтверждает [`detect_discharge_sign`] по показаниям на батарее: телефон без внешнего питания
//! всегда разряжается, значит знак тока в таких показаниях и есть знак разряда. Компромисс:
//! автоопределение требует нескольких ненулевых показаний без внешнего питания и явного
//! большинства одного знака (доля из конфига); без большинства (`Ambiguous`) сценарий замера
//! обязан остановиться, а не выбрать сам.
//!
//! Единицы — вторая ловушка. Документация Android обещает `CURRENT_NOW` в микроамперах, но OnePlus 11
//! (OxygenOS 16, факт 07) отдаёт МИЛЛИамперы: ≈ 490 при работающей камере, системная строка Oplus
//! `dumpsys battery` «Battery current : 509» — те же миллиамперы. Поэтому единица сырого значения —
//! явный [`CurrentNowUnit`] в запросе опроса, платформа переводит в микроамперы до контракта, а
//! [`PlausibleCurrentRange`] ловит неверно выбранную единицу (ток в 1000 раз меньше или больше).

use std::num::NonZeroUsize;

use serde::Serialize;

use crate::clock::MonotonicNanos;

#[cfg(any(test, feature = "fake-power-source"))]
pub mod fake;
pub mod sampling;
#[cfg(test)]
mod tests;

pub use sampling::{
    PowerPushOutcome, PowerReadFailure, PowerSampleRing, PowerSampling, PowerSamplingPeriod, PowerSamplingRequest,
    PowerSamplingStats, PowerSource, PowerSourceError,
};

/// В каких единицах платформа отдаёт `CURRENT_NOW` (см. документацию модуля: вендорская особенность).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentNowUnit {
    /// Как в документации Android.
    Microamps,
    /// Как на OnePlus 11 (OxygenOS 16), факт 07.
    Milliamps,
}

impl CurrentNowUnit {
    /// Сырое значение платформы в микроамперах (с насыщением: переполнения на реальных токах нет).
    pub const fn to_microamps(self, raw: i64) -> i64 {
        match self {
            Self::Microamps => raw,
            Self::Milliamps => raw.saturating_mul(1_000),
        }
    }
}

/// Мгновенный ток батареи в микроамперах: значение платформы, ПЕРЕВЕДЁННОЕ в мкА по
/// [`CurrentNowUnit`], но со знаком производителя (см. документацию модуля). Для расчётов —
/// [`BatteryCurrent::discharge_current`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct BatteryCurrent(i64);

impl BatteryCurrent {
    pub const fn from_raw_microamps(raw_microamps: i64) -> Self {
        Self(raw_microamps)
    }

    /// Значение платформы в мкА, без поправки знака.
    pub const fn raw_microamps(self) -> i64 {
        self.0
    }

    /// Ток разряда при заданном соглашении о знаке: положительный — батарея отдаёт энергию.
    pub fn discharge_current(self, sign: DischargeSign) -> DischargeCurrent {
        let microamps = match sign {
            // Документация Android: разряд — отрицательный, значит разряд = −сырое.
            // saturating_neg: i64::MIN не превращается в панику переполнения.
            DischargeSign::NegativeWhenDischarging => self.0.saturating_neg(),
            DischargeSign::PositiveWhenDischarging => self.0,
        };
        DischargeCurrent(microamps)
    }
}

/// Ток разряда в микроамперах: положительный — батарея разряжается, отрицательный — заряжается.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct DischargeCurrent(i64);

impl DischargeCurrent {
    pub const fn microamps(self) -> i64 {
        self.0
    }

    /// В миллиамперах (для отчётов; точность f64 избыточна для диапазона батарей телефона).
    pub fn milliamps(self) -> f64 {
        const MICROAMPS_PER_MILLIAMP: f64 = 1_000.0;
        // Ток телефона ≪ 2^53 мкА, поэтому перевод в f64 точен.
        self.0 as f64 / MICROAMPS_PER_MILLIAMP
    }
}

/// Соглашение о знаке `CURRENT_NOW` при разряде.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DischargeSign {
    /// Как в документации Android: ток в батарею положительный, разряд — отрицательный.
    NegativeWhenDischarging,
    /// Инвертировано производителем: разряд — положительный.
    PositiveWhenDischarging,
}

/// Что показали показания на батарее о знаке тока разряда.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "evidence", rename_all = "snake_case")]
pub enum DischargeSignEvidence {
    /// Не меньше доли [`SignAgreement`] годных ненулевых показаний одного знака — соглашение
    /// установлено; `opposing` — сколько показаний было против (шум вендора около нуля).
    Detected {
        sign: DischargeSign,
        samples: usize,
        opposing: usize,
    },
    /// Ни один знак не набрал нужной доли — знак не установить (например, вендор шумит около
    /// нуля или телефон начал заряжаться без смены статуса). Мерить нельзя, пока соглашение не
    /// задано явно.
    Ambiguous { positive: usize, negative: usize },
    /// Годных ненулевых показаний меньше требуемого.
    InsufficientSamples { usable: usize, required: usize },
}

/// Какая доля годных ненулевых показаний должна иметь один знак, чтобы считать его знаком
/// разряда. Строго больше половины: при ничьей и около неё знак не угадывается никогда.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignAgreement(f64);

impl SignAgreement {
    /// Доля `fraction` в `(0,5; 1]`; иначе (включая NaN) — `None`.
    pub fn new(fraction: f64) -> Option<Self> {
        // Через сравнения: NaN не проходит ни одно.
        (fraction > 0.5 && fraction <= 1.0).then_some(Self(fraction))
    }

    pub fn fraction(self) -> f64 {
        self.0
    }

    /// Набирает ли `majority` из `usable` нужную долю.
    fn is_reached_by(self, majority: usize, usable: usize) -> bool {
        // Число показаний ≪ 2^53, перевод в f64 точен.
        usable > 0 && majority as f64 >= self.0 * usable as f64
    }
}

/// Правило определения знака: сколько показаний нужно и какая доля должна согласиться.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignDetectionRule {
    /// Минимум годных ненулевых показаний.
    pub required: NonZeroUsize,
    /// Доля показаний, которые обязаны иметь один знак.
    pub agreement: SignAgreement,
}

/// Определяет знак разряда по показаниям: учитываются только годные для замера
/// ([`MeasurementValidity::Valid`], то есть без внешнего питания) с ненулевым током.
///
/// Правило большинства: знак установлен, если показаний не меньше `rule.required` и доля
/// большинства не меньше `rule.agreement` (по умолчанию в конфиге 95 %). Одиночные показания
/// другого знака (шум около нуля у некоторых вендоров) не ломают замер, но разделение без
/// явного большинства — `Ambiguous`, а не догадка.
pub fn detect_discharge_sign(samples: &[BatterySample], rule: SignDetectionRule) -> DischargeSignEvidence {
    let mut positive = 0_usize;
    let mut negative = 0_usize;
    let usable_currents = samples
        .iter()
        .filter(|sample| sample.measurement_validity() == MeasurementValidity::Valid)
        .filter_map(|sample| sample.current);
    for current in usable_currents {
        match current.raw_microamps().signum() {
            1 => positive += 1,
            -1 => negative += 1,
            // Ноль знака не несёт: телефон с включённым экраном всегда потребляет ток,
            // ноль — признак «датчик не обновился», а не разряда.
            _ => {}
        }
    }
    let usable = positive + negative;
    let (sign, majority, opposing) = if positive >= negative {
        (DischargeSign::PositiveWhenDischarging, positive, negative)
    } else {
        (DischargeSign::NegativeWhenDischarging, negative, positive)
    };
    if opposing > 0 && !rule.agreement.is_reached_by(majority, usable) {
        return DischargeSignEvidence::Ambiguous { positive, negative };
    }
    if usable < rule.required.get() {
        return DischargeSignEvidence::InsufficientSamples {
            usable,
            required: rule.required.get(),
        };
    }
    DischargeSignEvidence::Detected {
        sign,
        samples: usable,
        opposing,
    }
}

/// Правдоподобный диапазон модуля среднего тока телефона на батарее, мкА.
///
/// Зачем: `CURRENT_NOW` по документации — микроамперы, но ядра некоторых вендоров (встречалось у
/// OnePlus/Oppo) отдают миллиамперы. Тогда 500 мА читаются как 0,5 мА, и замер «стадия стоит
/// 0,02 мА» был бы в тысячу раз неверен, а не просто шумен. Проверка после перевода единиц
/// превращает такое в явный отказ (`UnitSuspect` у потребителя), а не в тихое число.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlausibleCurrentRange {
    min_microamps: u64,
    max_microamps: u64,
}

/// Где среднее окна относительно правдоподобного диапазона.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentPlausibility {
    Plausible,
    /// Меньше нижней границы: похоже на миллиамперы вместо микроампер.
    BelowRange,
    /// Больше верхней границы: похоже на наноамперы или мусор.
    AboveRange,
}

impl PlausibleCurrentRange {
    /// Диапазон `[min_ma, max_ma]` мА; `None`, если `min_ma = 0`, `min_ma ≥ max_ma` или
    /// перевод в мкА переполняется.
    pub fn from_milliamps(min_ma: u64, max_ma: u64) -> Option<Self> {
        const MICROAMPS_PER_MILLIAMP: u64 = 1_000;
        if min_ma == 0 || min_ma >= max_ma {
            return None;
        }
        Some(Self {
            min_microamps: min_ma.checked_mul(MICROAMPS_PER_MILLIAMP)?,
            max_microamps: max_ma.checked_mul(MICROAMPS_PER_MILLIAMP)?,
        })
    }

    pub fn min_microamps(self) -> u64 {
        self.min_microamps
    }

    pub fn max_microamps(self) -> u64 {
        self.max_microamps
    }

    /// Правдоподобен ли модуль среднего тока `mean_abs_microamps` (NaN — неправдоподобен).
    pub fn classify(self, mean_abs_microamps: f64) -> CurrentPlausibility {
        // Границы ≪ 2^53 мкА, перевод в f64 точен.
        if mean_abs_microamps.is_nan() || mean_abs_microamps < self.min_microamps as f64 {
            CurrentPlausibility::BelowRange
        } else if mean_abs_microamps > self.max_microamps as f64 {
            CurrentPlausibility::AboveRange
        } else {
            CurrentPlausibility::Plausible
        }
    }
}

/// Счётчик заряда батареи в микроампер-часах (`BATTERY_PROPERTY_CHARGE_COUNTER`).
/// Разность двух показаний = израсходованный заряд за интервал.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct MicroAmpHours(i64);

impl MicroAmpHours {
    pub const fn from_microamp_hours(value: i64) -> Self {
        Self(value)
    }

    pub const fn microamp_hours(self) -> i64 {
        self.0
    }

    /// Сколько заряда ушло от `earlier` до `self` (положительное — батарея потеряла заряд).
    pub fn consumed_since(self, earlier: MicroAmpHours) -> i64 {
        earlier.0.saturating_sub(self.0)
    }
}

/// Температура батареи в десятых долях °C (`EXTRA_TEMPERATURE`): 312 = 31,2 °C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct DeciCelsius(i32);

impl DeciCelsius {
    pub const fn from_deci_celsius(value: i32) -> Self {
        Self(value)
    }

    pub const fn deci_celsius(self) -> i32 {
        self.0
    }

    pub fn celsius(self) -> f32 {
        const DECI_PER_DEGREE: f32 = 10.0;
        // Диапазон температур батареи (сотни десятых) точно представим в f32.
        self.0 as f32 / DECI_PER_DEGREE
    }
}

/// Напряжение батареи в милливольтах (`EXTRA_VOLTAGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct MilliVolts(u32);

impl MilliVolts {
    pub const fn from_millivolts(value: u32) -> Self {
        Self(value)
    }

    pub const fn millivolts(self) -> u32 {
        self.0
    }
}

/// Состояние заряда (`BatteryManager.BATTERY_STATUS_*` с поправкой на `isCharging`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargingState {
    Charging,
    Discharging,
    /// Питание подключено, но заряд не идёт (лимит заряда, температура) — или статус вендора.
    NotCharging,
    Full,
    Unknown,
}

/// Источник внешнего питания (`EXTRA_PLUGGED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerPlug {
    /// На батарее (`EXTRA_PLUGGED == 0`).
    Unplugged,
    Ac,
    Usb,
    Wireless,
    Dock,
    /// Значение нет или оно не из известных: нельзя доказать, что телефон на батарее.
    Unknown,
}

/// Годится ли показание для замера тока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "validity", rename_all = "snake_case")]
pub enum MeasurementValidity {
    /// Телефон на батарее: ток батареи = ток телефона.
    Valid,
    /// Внешнее питание: ток батареи — разность зарядки и потребления, вклад стадии не виден.
    RejectedExternalPower { plug: PowerPlug, charging: ChargingState },
    /// Платформа не сказала, подключено ли питание: честно не мерим.
    RejectedPowerStateUnknown { charging: ChargingState },
}

/// Одно показание батареи.
///
/// Поля `Option` — свойство не поддержано платформой или вендором (`Long.MIN_VALUE` /
/// `Integer.MIN_VALUE` у Android), а не ноль.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BatterySample {
    /// Момент чтения по `CLOCK_MONOTONIC` (`System.nanoTime`).
    pub at: MonotonicNanos,
    pub current: Option<BatteryCurrent>,
    pub charge_counter: Option<MicroAmpHours>,
    pub charging: ChargingState,
    pub plug: PowerPlug,
    pub temperature: Option<DeciCelsius>,
    pub level_percent: Option<u8>,
    pub voltage: Option<MilliVolts>,
}

impl BatterySample {
    /// Можно ли по этому показанию мерить ток телефона.
    ///
    /// Правило: решает источник питания; «на батарее, но статус Charging» — противоречие
    /// платформы, и тогда тоже отказ (энергия идёт в батарею). `Full` или `NotCharging` на
    /// батарее допустимы: Android держит `Full`, пока уровень не упадёт, а разряд уже идёт.
    pub fn measurement_validity(&self) -> MeasurementValidity {
        match (self.plug, self.charging) {
            (PowerPlug::Unplugged, ChargingState::Charging) => MeasurementValidity::RejectedExternalPower {
                plug: self.plug,
                charging: self.charging,
            },
            (PowerPlug::Unplugged, _) => MeasurementValidity::Valid,
            (PowerPlug::Unknown, charging) => MeasurementValidity::RejectedPowerStateUnknown { charging },
            (plug, charging) => MeasurementValidity::RejectedExternalPower { plug, charging },
        }
    }
}
