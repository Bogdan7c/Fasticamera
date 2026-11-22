//! Смещение местного времени от UTC через `localtime_r` (POSIX; есть в glibc и в Bionic).
//!
//! Единственное место `unsafe` для часов снимка: вызов C-функции с корректными указателями.

use std::mem::MaybeUninit;

/// Смещение пояса процесса от UTC в секундах на момент `unix_seconds` (с учётом летнего времени).
/// `None` — время не представимо в `time_t` или `localtime_r` отказала.
pub(super) fn local_utc_offset_seconds(unix_seconds: i64) -> Option<i32> {
    let time = libc::time_t::try_from(unix_seconds).ok()?;
    let mut broken_down = MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `time` — живая локальная переменная, `broken_down` — выделенная под `tm` память нужного размера;
    // `localtime_r` потокобезопасна (пишет только в переданный буфер) и при успехе полностью заполняет `tm`.
    let filled = unsafe { libc::localtime_r(&raw const time, broken_down.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }
    // SAFETY: возврат не NULL — `localtime_r` заполнила структуру (до вызова она была обнулена, поле
    // `tm_zone` — нулевой указатель, что допустимо для `tm`).
    let broken_down = unsafe { broken_down.assume_init() };
    i32::try_from(broken_down.tm_gmtoff).ok()
}
