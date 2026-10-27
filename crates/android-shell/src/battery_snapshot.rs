//! Раскладка снимка батареи Kotlin → Rust и его разбор в нейтральный [`BatterySample`].
//!
//! `ShellPlatform.readBatterySnapshot()` за один JNI-вызов отдаёт `LongArray` с сырыми
//! значениями платформы (слоты ниже). Kotlin ничего не решает: он только читает
//! `BatteryManager.getLongProperty`, `isCharging` и липкое `ACTION_BATTERY_CHANGED`. Всё
//! толкование (неподдержанное свойство → `None`, коды статуса и источника питания, процент
//! уровня) — здесь, в нейтральном коде, который тестируется на ПК.
//!
//! Контракт «нет значения» (документация Android): `getLongProperty` возвращает
//! `Long.MIN_VALUE`, а отсутствующий extra Kotlin заменяет на `Integer.MIN_VALUE`. Оба значения
//! означают «свойство не поддержано» и превращаются в `None`, а не в ноль.

use shell_contract::clock::MonotonicNanos;
use shell_contract::power::{
    BatteryCurrent, BatterySample, ChargingState, CurrentNowUnit, DeciCelsius, MicroAmpHours, MilliVolts, PowerPlug,
};

/// Версия раскладки; Kotlin кладёт её в слот 0 (`ShellPlatform.BATTERY_SNAPSHOT_LAYOUT_VERSION`).
pub const BATTERY_SNAPSHOT_LAYOUT_VERSION: i64 = 1;

/// Номера слотов `LongArray` — контракт с `ShellPlatform.readBatterySnapshot()`.
pub mod slot {
    /// Версия раскладки.
    pub const LAYOUT_VERSION: usize = 0;
    /// `System.nanoTime()` сразу после чтения тока (`CLOCK_MONOTONIC`).
    pub const SAMPLED_AT_MONOTONIC_NANOS: usize = 1;
    /// `getLongProperty(BATTERY_PROPERTY_CURRENT_NOW)`, мкА, знак вендора.
    /// `CURRENT_NOW` в единицах устройства (по документации мкА, на OnePlus 11 — мА).
    pub const CURRENT_NOW_MICROAMPS: usize = 2;
    /// `getLongProperty(BATTERY_PROPERTY_CHARGE_COUNTER)`, мкА·ч.
    pub const CHARGE_COUNTER_MICROAMP_HOURS: usize = 3;
    /// `EXTRA_STATUS` (`BATTERY_STATUS_*`).
    pub const STATUS: usize = 4;
    /// `EXTRA_PLUGGED` (0 — батарея, `BATTERY_PLUGGED_*`).
    pub const PLUGGED: usize = 5;
    /// `EXTRA_TEMPERATURE`, десятые °C.
    pub const TEMPERATURE_DECI_CELSIUS: usize = 6;
    /// `EXTRA_LEVEL`.
    pub const LEVEL: usize = 7;
    /// `EXTRA_SCALE` (максимум уровня).
    pub const SCALE: usize = 8;
    /// `EXTRA_VOLTAGE`, мВ.
    pub const VOLTAGE_MILLIVOLTS: usize = 9;
    /// `BatteryManager.isCharging()`: 1 — да, 0 — нет.
    pub const IS_CHARGING: usize = 10;
    /// Сколько слотов в снимке.
    pub const COUNT: usize = 11;
}

/// `Long.MIN_VALUE`: `getLongProperty` не поддержано.
const JAVA_LONG_UNSUPPORTED: i64 = i64::MIN;
/// `Integer.MIN_VALUE`: extra отсутствует (или свойство через int-путь не поддержано).
const JAVA_INT_UNSUPPORTED: i64 = i32::MIN as i64;

/// Коды `BatteryManager.BATTERY_STATUS_*` (android.jar).
const STATUS_UNKNOWN: i64 = 1;
const STATUS_CHARGING: i64 = 2;
const STATUS_DISCHARGING: i64 = 3;
const STATUS_NOT_CHARGING: i64 = 4;
const STATUS_FULL: i64 = 5;

/// Коды `EXTRA_PLUGGED`: 0 — на батарее, иначе `BatteryManager.BATTERY_PLUGGED_*`.
const PLUGGED_NONE: i64 = 0;
const PLUGGED_AC: i64 = 1;
const PLUGGED_USB: i64 = 2;
const PLUGGED_WIRELESS: i64 = 4;
/// `BATTERY_PLUGGED_DOCK`, API 33.
const PLUGGED_DOCK: i64 = 8;

/// Значение слота `IS_CHARGING`, означающее «заряжается».
const IS_CHARGING_TRUE: i64 = 1;

const PERCENT_SCALE: i64 = 100;

/// Почему снимок не разобран (несовпадение раскладки Kotlin ↔ Rust или мусор в слоте).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BatterySnapshotError {
    #[error("в снимке {actual} слотов, ожидалось {expected}")]
    WrongSlotCount { expected: usize, actual: usize },
    #[error("версия раскладки снимка {actual}, ожидалась {expected}")]
    UnsupportedLayoutVersion { expected: i64, actual: i64 },
    #[error("слот {slot}: значение {value} не помещается в int Java")]
    IntSlotOutOfRange { slot: &'static str, value: i64 },
}

impl BatterySnapshotError {
    /// Ошибка длины для вызывающего, который проверил длину массива до копирования.
    pub fn wrong_slot_count(actual: usize) -> Self {
        Self::WrongSlotCount {
            expected: slot::COUNT,
            actual,
        }
    }
}

/// Разбирает снимок `ShellPlatform.readBatterySnapshot()` в показание.
pub fn decode_battery_snapshot(
    slots: &[i64],
    current_now_unit: CurrentNowUnit,
) -> Result<BatterySample, BatterySnapshotError> {
    let slots: &[i64; slot::COUNT] = slots
        .try_into()
        .map_err(|_| BatterySnapshotError::wrong_slot_count(slots.len()))?;
    let version = slots[slot::LAYOUT_VERSION];
    if version != BATTERY_SNAPSHOT_LAYOUT_VERSION {
        return Err(BatterySnapshotError::UnsupportedLayoutVersion {
            expected: BATTERY_SNAPSHOT_LAYOUT_VERSION,
            actual: version,
        });
    }
    let status = int_slot(slots, slot::STATUS, "status")?;
    let plugged = int_slot(slots, slot::PLUGGED, "plugged")?;
    let temperature = int_slot(slots, slot::TEMPERATURE_DECI_CELSIUS, "temperature")?;
    let level = int_slot(slots, slot::LEVEL, "level")?;
    let scale = int_slot(slots, slot::SCALE, "scale")?;
    let voltage = int_slot(slots, slot::VOLTAGE_MILLIVOLTS, "voltage")?;
    let platform_says_charging = slots[slot::IS_CHARGING] == IS_CHARGING_TRUE;
    Ok(BatterySample {
        at: MonotonicNanos::from_nanos(slots[slot::SAMPLED_AT_MONOTONIC_NANOS]),
        // Единица сырого значения — вендорская (OnePlus 11 отдаёт мА), в контракт идут мкА.
        current: supported(slots[slot::CURRENT_NOW_MICROAMPS])
            .map(|raw| BatteryCurrent::from_raw_microamps(current_now_unit.to_microamps(raw))),
        charge_counter: supported(slots[slot::CHARGE_COUNTER_MICROAMP_HOURS]).map(MicroAmpHours::from_microamp_hours),
        charging: charging_state(status, platform_says_charging),
        plug: power_plug(plugged),
        temperature: temperature.map(DeciCelsius::from_deci_celsius),
        level_percent: level_percent(level, scale),
        voltage: voltage
            .and_then(|millivolts| u32::try_from(millivolts).ok())
            .filter(|millivolts| *millivolts > 0)
            .map(MilliVolts::from_millivolts),
    })
}

/// `None` для обоих маркеров «не поддержано» (`Long.MIN_VALUE`, `Integer.MIN_VALUE`).
fn supported(value: i64) -> Option<i64> {
    (value != JAVA_LONG_UNSUPPORTED && value != JAVA_INT_UNSUPPORTED).then_some(value)
}

/// Слот, который в Java — `int`: маркер отсутствия → `None`, выход за `i32` — ошибка раскладки.
fn int_slot(slots: &[i64; slot::COUNT], index: usize, name: &'static str) -> Result<Option<i32>, BatterySnapshotError> {
    let value = slots[index];
    if value == JAVA_INT_UNSUPPORTED {
        return Ok(None);
    }
    i32::try_from(value)
        .map(Some)
        .map_err(|_| BatterySnapshotError::IntSlotOutOfRange { slot: name, value })
}

/// Статус заряда. `isCharging()` — отдельное мнение платформы о заряде: если оно «да», а
/// `EXTRA_STATUS` говорит иное (вендорские статусы бывают отстающими), побеждает «заряжается».
/// Ошибиться в сторону «заряжается» безопасно: замер тогда честно откажется.
fn charging_state(status: Option<i32>, platform_says_charging: bool) -> ChargingState {
    let from_status = match status.map(i64::from) {
        Some(STATUS_CHARGING) => ChargingState::Charging,
        Some(STATUS_DISCHARGING) => ChargingState::Discharging,
        Some(STATUS_NOT_CHARGING) => ChargingState::NotCharging,
        Some(STATUS_FULL) => ChargingState::Full,
        Some(STATUS_UNKNOWN) | Some(_) | None => ChargingState::Unknown,
    };
    match from_status {
        ChargingState::Charging | ChargingState::Full => from_status,
        _ if platform_says_charging => ChargingState::Charging,
        _ => from_status,
    }
}

/// Источник питания. Незнакомый ненулевой код (будущий тип или битовая смесь) — `Unknown`:
/// доказать «на батарее» нельзя, и замер откажется.
fn power_plug(plugged: Option<i32>) -> PowerPlug {
    match plugged.map(i64::from) {
        Some(PLUGGED_NONE) => PowerPlug::Unplugged,
        Some(PLUGGED_AC) => PowerPlug::Ac,
        Some(PLUGGED_USB) => PowerPlug::Usb,
        Some(PLUGGED_WIRELESS) => PowerPlug::Wireless,
        Some(PLUGGED_DOCK) => PowerPlug::Dock,
        Some(_) | None => PowerPlug::Unknown,
    }
}

/// Уровень в процентах: `level × 100 / scale` с округлением к ближайшему, не больше 100.
fn level_percent(level: Option<i32>, scale: Option<i32>) -> Option<u8> {
    let level = i64::from(level?);
    let scale = i64::from(scale?);
    if level < 0 || scale <= 0 {
        return None;
    }
    // Округление к ближайшему целым: (level·100 + scale/2) / scale.
    let percent = (level * PERCENT_SCALE + scale / 2) / scale;
    u8::try_from(percent.min(PERCENT_SCALE)).ok()
}

#[cfg(test)]
#[path = "battery_snapshot_tests.rs"]
mod tests;
