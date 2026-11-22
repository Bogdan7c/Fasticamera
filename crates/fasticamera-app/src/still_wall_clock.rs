//! Настенные часы для файлов снимка: UTC в миллисекундах и смещение местного времени (`WallClock` службы записи).
//!
//! UTC — системные часы (`SystemTime`); смещение — часовой пояс процесса через `localtime_r` (`tm_gmtoff`), одним
//! кодом на Android и на ПК. Опасная часть (FFI) — в [`local_offset`], здесь только правила.
//!
//! Если пояс определить не удалось, смещение — 0 (UTC) и это ОДИН раз громко пишется в лог: файл всё равно
//! получает верный UTC-момент, а `OffsetTimeOriginal` честно скажет «+00:00», не выдумывая пояс.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use photo_output::exif::CaptureMoment;
use photo_output::service::WallClock;

mod local_offset;

const LOG_TARGET: &str = "fasticamera::still::clock";

/// Настенные часы системы.
#[derive(Default)]
pub struct SystemWallClock {
    /// Сбой определения пояса уже записан в лог (пишем один раз: часы зовут на каждый снимок).
    offset_failure_logged: AtomicBool,
}

/// Миллисекунды Unix-времени из системных часов. Часы до 1970 года — отрицательное значение, не паника.
fn unix_millis(now: SystemTime) -> i64 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(after_epoch) => i64::try_from(after_epoch.as_millis()).unwrap_or(i64::MAX),
        Err(before_epoch) => i64::try_from(before_epoch.duration().as_millis()).map_or(i64::MIN, |millis| -millis),
    }
}

/// Момент съёмки из UTC и смещения пояса в секундах (`None` — пояс неизвестен, считаем UTC).
///
/// Смещение хранится в минутах (формат Exif `OffsetTimeOriginal`: `±HH:MM`), поэтому секунды ниже минуты
/// отбрасываются — исторических поясов с секундами в современных часах нет.
pub fn moment_from(utc_millis: i64, offset_seconds: Option<i32>) -> CaptureMoment {
    let minutes = offset_seconds.map_or(0, |seconds| seconds / 60);
    CaptureMoment {
        utc_millis,
        // Пояса лежат в пределах ±14 ч (±840 мин), в `i16` влезают с огромным запасом.
        utc_offset_minutes: i16::try_from(minutes).unwrap_or(0),
    }
}

impl WallClock for SystemWallClock {
    fn now(&self) -> CaptureMoment {
        let utc_millis = unix_millis(SystemTime::now());
        let offset_seconds = local_offset::local_utc_offset_seconds(utc_millis.div_euclid(1000));
        if offset_seconds.is_none() && !self.offset_failure_logged.swap(true, Ordering::Relaxed) {
            tracing::error!(target: LOG_TARGET, "часовой пояс не определён (localtime_r): смещение записано как UTC");
        }
        moment_from(utc_millis, offset_seconds)
    }
}

#[cfg(test)]
#[path = "still_wall_clock_tests.rs"]
mod tests;
