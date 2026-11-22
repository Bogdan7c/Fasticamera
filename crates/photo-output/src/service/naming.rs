//! Имена файлов снимка: `IMG_YYYYMMDD_HHMMSS_mmm.HEIC/JPG` и тот же корень для `.DNG`.
//!
//! Дата берётся по местному времени момента съёмки (UTC + смещение). Календарь свой (алгоритм
//! гражданской даты Говарда Хиннанта), чтобы не тянуть зависимость ради одной функции.

use still_contract::plan::PrimaryFormat;

use crate::exif::CaptureMoment;

/// MIME-тип DNG для хранилища.
pub(super) const DNG_MIME_TYPE: &str = "image/x-adobe-dng";
/// Расширение DNG.
pub(super) const DNG_EXTENSION: &str = "DNG";

const MILLIS_PER_MINUTE: i64 = 60_000;
const MILLIS_PER_DAY: i64 = 86_400_000;

/// Имена файлов одного снимка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StillFileNames {
    pub(super) primary: String,
    pub(super) dng: String,
}

impl StillFileNames {
    pub(super) fn for_moment(moment: &CaptureMoment, primary: PrimaryFormat) -> Self {
        let stem = file_stem(moment);
        Self {
            primary: format!("{stem}.{}", primary.file_extension()),
            dng: format!("{stem}.{DNG_EXTENSION}"),
        }
    }
}

/// Корень имени: `IMG_20261122_101530_123`.
pub(super) fn file_stem(moment: &CaptureMoment) -> String {
    let local_millis = moment
        .utc_millis
        .saturating_add(i64::from(moment.utc_offset_minutes) * MILLIS_PER_MINUTE);
    let days = local_millis.div_euclid(MILLIS_PER_DAY);
    let millis_of_day = local_millis.rem_euclid(MILLIS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = millis_of_day / 3_600_000;
    let minute = millis_of_day / MILLIS_PER_MINUTE % 60;
    let second = millis_of_day / 1_000 % 60;
    let millis = millis_of_day % 1_000;
    format!("IMG_{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}_{millis:03}")
}

/// Число дней от 1970-01-01 → (год, месяц, день) пролептического григорианского календаря.
/// Алгоритм `civil_from_days` (H. Hinnant): эпоха — 400-летний цикл, год считается от марта.
fn civil_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_unix_epoch + 719_468; // дни от 0000-03-01
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153; // 0 = март
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(utc_millis: i64, utc_offset_minutes: i16) -> CaptureMoment {
        CaptureMoment {
            utc_millis,
            utc_offset_minutes,
        }
    }

    #[test]
    fn unix_epoch_formats_as_1970() {
        assert_eq!(file_stem(&moment(0, 0)), "IMG_19700101_000000_000");
    }

    #[test]
    fn known_instant_formats_with_millis() {
        // 2026-11-22 10:15:30.123 UTC.
        assert_eq!(file_stem(&moment(1_795_342_530_123, 0)), "IMG_20261122_101530_123");
    }

    #[test]
    fn positive_offset_moves_to_next_local_day() {
        // 2026-11-22 23:30:00.000 UTC при UTC+03:00 — уже 23 ноября 02:30 местного.
        assert_eq!(file_stem(&moment(1_795_390_200_000, 180)), "IMG_20261123_023000_000");
    }

    #[test]
    fn negative_offset_moves_to_previous_day() {
        // 1970-01-01 01:00 UTC при UTC-02:00 — 1969-12-31 23:00.
        assert_eq!(file_stem(&moment(3_600_000, -120)), "IMG_19691231_230000_000");
    }

    #[test]
    fn leap_day_is_kept() {
        // 2024-02-29 12:00:00 UTC.
        assert_eq!(file_stem(&moment(1_709_208_000_000, 0)), "IMG_20240229_120000_000");
    }

    #[test]
    fn names_share_stem_and_differ_by_extension() {
        let names = StillFileNames::for_moment(&moment(0, 0), PrimaryFormat::Jpeg);
        assert_eq!(names.primary, "IMG_19700101_000000_000.JPG");
        assert_eq!(names.dng, "IMG_19700101_000000_000.DNG");
        let heif = StillFileNames::for_moment(&moment(0, 0), PrimaryFormat::Heif);
        assert_eq!(heif.primary, "IMG_19700101_000000_000.HEIC");
    }
}
