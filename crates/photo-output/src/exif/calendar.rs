//! Календарь без внешних крейтов: перевод Unix-времени в «настенные» дату и время.
//!
//! Алгоритм `civil_from_days` — Howard Hinnant, «chrono-Compatible Low-Level Date Algorithms»
//! (пролептический григорианский календарь, работает для любых дней, включая до 1970 года).

/// Разложенное по полям местное время.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClock {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millisecond: u16,
}

const MILLIS_PER_SECOND: i64 = 1000;
const MILLIS_PER_DAY: i64 = 86_400_000;
const MILLIS_PER_MINUTE: i64 = 60_000;

/// Местное время для момента UTC и смещения зоны в минутах. `None` — арифметика переполнилась.
pub fn wall_clock(utc_millis: i64, utc_offset_minutes: i16) -> Option<WallClock> {
    let local_millis = utc_millis.checked_add(i64::from(utc_offset_minutes) * MILLIS_PER_MINUTE)?;
    // Евклидово деление: для моментов до 1970 года остаток времени суток остаётся неотрицательным.
    let days_since_epoch = local_millis.div_euclid(MILLIS_PER_DAY);
    let millis_of_day = local_millis.rem_euclid(MILLIS_PER_DAY);
    let (year, month, day) = civil_from_days(days_since_epoch);

    let seconds_of_day = millis_of_day / MILLIS_PER_SECOND;
    Some(WallClock {
        year,
        month,
        day,
        hour: u8::try_from(seconds_of_day / 3600).ok()?,
        minute: u8::try_from(seconds_of_day % 3600 / 60).ok()?,
        second: u8::try_from(seconds_of_day % 60).ok()?,
        millisecond: u16::try_from(millis_of_day % MILLIS_PER_SECOND).ok()?,
    })
}

/// Дни от 1970-01-01 → (год, месяц 1–12, день 1–31).
fn civil_from_days(days_since_epoch: i64) -> (i64, u8, u8) {
    // Сдвигаем начало отсчёта на 0000-03-01: тогда високосный день — последний в «году».
    let shifted = days_since_epoch + 719_468;
    let era = shifted.div_euclid(146_097); // 400 лет = 146097 дней
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153; // 0 = март … 11 = февраль
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // month ∈ 1..=12 и day ∈ 1..=31 по построению алгоритма; приведение безопасно.
    (year, month as u8, day as u8)
}
