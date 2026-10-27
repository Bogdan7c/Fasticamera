//! Смещение между часами `BOOTTIME` и `MONOTONIC`: мост для замера задержки «сенсор -> дисплей».
//!
//! Метка сенсора кадра (`SENSOR_TIMESTAMP`) идёт в шкале `CLOCK_BOOTTIME`, а реальный момент показа
//! кадра на экране (`VK_GOOGLE_display_timing`) — в `CLOCK_MONOTONIC`. Эти часы расходятся на время,
//! проведённое телефоном во сне, и молча сравнивать их нельзя (`shell_contract::clock`). Мост —
//! смещение `BOOTTIME - MONOTONIC`, измеренное в один момент: пока телефон не засыпает, оно
//! постоянно, а во время замера экран включён.
//!
//! * [`ClockOffset`] — чистое значение с арифметикой, тестируется без телефона;
//! * [`ClockOffsetSource`] — узкий контракт «дай смещение» (в тестах подставляется fake);
//! * [`SystemClocks`] — боевая реализация через `clock_gettime` (Linux и Android).

use shell_contract::clock::{BoottimeNanos, MonotonicNanos};

/// Смещение `BOOTTIME - MONOTONIC` в наносекундах, измеренное в один момент.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockOffset {
    boottime_minus_monotonic_nanos: i64,
}

impl ClockOffset {
    /// Смещение по двум показаниям, снятым в один и тот же момент.
    pub fn between(boottime: BoottimeNanos, monotonic: MonotonicNanos) -> Self {
        Self {
            boottime_minus_monotonic_nanos: boottime.as_nanos().saturating_sub(monotonic.as_nanos()),
        }
    }

    /// Тот же момент времени в шкале `BOOTTIME`.
    pub fn monotonic_to_boottime(self, monotonic: MonotonicNanos) -> BoottimeNanos {
        BoottimeNanos::from_nanos(monotonic.as_nanos().saturating_add(self.boottime_minus_monotonic_nanos))
    }
}

/// Почему часы не удалось прочитать.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("clock_gettime({clock}) отказал, errno {errno}")]
pub struct ClockReadError {
    /// Какие часы читали.
    pub clock: &'static str,
    pub errno: i32,
}

/// Источник смещения часов (на телефоне — системные часы, в тестах — fake).
pub trait ClockOffsetSource: Send + Sync {
    /// Свежее смещение. Ошибка чтения — значение, а не паника: замер задержки тогда честно
    /// сообщает «недоступно».
    fn sample_offset(&self) -> Result<ClockOffset, ClockReadError>;
}

/// Системные часы Linux/Android.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClocks;

/// Одно чтение часов `clock_id` в наносекундах.
#[expect(
    clippy::useless_conversion,
    reason = "на 64-битных целях tv_sec и tv_nsec уже i64, на 32-битных нет: преобразование нужно для переносимости"
)]
fn read_clock_nanos(clock_id: libc::clockid_t, clock: &'static str) -> Result<i64, ClockReadError> {
    let mut reading = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `reading` — живая локальная переменная нужного типа, `clock_gettime` пишет только в неё.
    let status = unsafe { libc::clock_gettime(clock_id, &mut reading) };
    if status != 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        return Err(ClockReadError { clock, errno });
    }
    Ok(i64::from(reading.tv_sec)
        .saturating_mul(1_000_000_000)
        .saturating_add(i64::from(reading.tv_nsec)))
}

/// Сколько «вилок» `MONOTONIC → BOOTTIME → MONOTONIC` снимать за одно смещение.
///
/// Ошибка смещения — половина зазора между двумя чтениями `MONOTONIC`. Обычно это единицы
/// микросекунд, но если поток вытеснили посреди вилки (нагруженный ПК, фоновые потоки телефона),
/// зазор вырастает до миллисекунд — и смещение, а с ним и замер задержки «сенсор → экран», уплывает
/// (найдено в 07: тест смещения изредка падал под параллельными GPU-тестами). Из нескольких вилок
/// берём самую тесную: вероятность вытеснения во всех пяти подряд пренебрежимо мала, а цена —
/// 15 вызовов `clock_gettime` (vDSO, доли микросекунды каждый).
const CLOCK_BRACKET_ATTEMPTS: usize = 5;

/// Одна вилка чтений: `MONOTONIC` до, `BOOTTIME`, `MONOTONIC` после (нс).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClockBracket {
    monotonic_before: i64,
    boottime: i64,
    monotonic_after: i64,
}

impl ClockBracket {
    /// Зазор между чтениями `MONOTONIC`: удвоенная граница ошибки смещения.
    fn gap_nanos(self) -> i64 {
        self.monotonic_after.saturating_sub(self.monotonic_before)
    }

    /// Смещение по середине вилки: ошибка от времени между чтениями делится пополам.
    fn offset(self) -> ClockOffset {
        let monotonic_middle = self.monotonic_before + self.gap_nanos() / 2;
        ClockOffset::between(
            BoottimeNanos::from_nanos(self.boottime),
            MonotonicNanos::from_nanos(monotonic_middle),
        )
    }
}

/// Снимает `attempts` вилок через `read_bracket` и возвращает смещение самой тесной.
/// Ошибка чтения любой вилки — ошибка всего замера (молча не пропускаем).
fn tightest_bracket_offset(
    attempts: usize,
    mut read_bracket: impl FnMut() -> Result<ClockBracket, ClockReadError>,
) -> Result<ClockOffset, ClockReadError> {
    let mut tightest = read_bracket()?;
    for _ in 1..attempts {
        let candidate = read_bracket()?;
        if candidate.gap_nanos() < tightest.gap_nanos() {
            tightest = candidate;
        }
    }
    Ok(tightest.offset())
}

/// Одна вилка системных часов.
fn read_system_bracket() -> Result<ClockBracket, ClockReadError> {
    let monotonic_before = read_clock_nanos(libc::CLOCK_MONOTONIC, "CLOCK_MONOTONIC")?;
    let boottime = read_clock_nanos(libc::CLOCK_BOOTTIME, "CLOCK_BOOTTIME")?;
    let monotonic_after = read_clock_nanos(libc::CLOCK_MONOTONIC, "CLOCK_MONOTONIC")?;
    Ok(ClockBracket {
        monotonic_before,
        boottime,
        monotonic_after,
    })
}

impl ClockOffsetSource for SystemClocks {
    /// Самая тесная из [`CLOCK_BRACKET_ATTEMPTS`] вилок `MONOTONIC → BOOTTIME → MONOTONIC`,
    /// смещение — по её середине.
    fn sample_offset(&self) -> Result<ClockOffset, ClockReadError> {
        tightest_bracket_offset(CLOCK_BRACKET_ATTEMPTS, read_system_bracket)
    }
}

/// Текущий момент `CLOCK_MONOTONIC` — шкала показаний батареи (`BatterySample::at`) и вход
/// `on_tick` замеров тока (`power_measurement`).
pub fn monotonic_now() -> Result<MonotonicNanos, ClockReadError> {
    read_clock_nanos(libc::CLOCK_MONOTONIC, "CLOCK_MONOTONIC").map(MonotonicNanos::from_nanos)
}

#[cfg(test)]
#[path = "clock_offset_tests.rs"]
mod tests;
