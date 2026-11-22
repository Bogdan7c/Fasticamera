//! Часы снимка читают часовой пояс ПРОЦЕССА: пояс задаётся переменной `TZ` до первого запроса времени.
//!
//! Отдельный бинарь теста с единственным тестом: `TZ` влияет на весь процесс, в общем бинаре смена пояса
//! гонялась бы с другими тестами (а glibc читает `TZ` один раз — поэтому и проверка одна).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use fasticamera::still_wall_clock::SystemWallClock;
use photo_output::service::WallClock;

#[test]
fn posix_tz_string_gives_the_matching_offset_in_minutes() {
    // POSIX-формат: `IST-5:30` — пояс «IST» на 5:30 ВОСТОЧНЕЕ UTC (знак в POSIX обратный). Без базы tzdata.
    // SAFETY: тест один в бинаре и до установки `TZ` не запускает потоков, читающих окружение.
    unsafe { std::env::set_var("TZ", "IST-5:30") };
    assert_eq!(SystemWallClock::default().now().utc_offset_minutes, 330);
}
