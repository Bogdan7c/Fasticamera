//! Часы `CLOCK_BOOTTIME` для замера возраста кадра (`now − SENSOR_TIMESTAMP`) в слушателе reader-а.

use camera_contract::capture_result::SensorTimestamp;

use crate::reader_drain::BoottimeClock;

/// Настоящие часы Android: `clock_gettime(CLOCK_BOOTTIME)`. Без состояния, разделяются свободно.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SystemBoottimeClock;

impl BoottimeClock for SystemBoottimeClock {
    fn now(&self) -> Option<SensorTimestamp> {
        let mut spec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `spec` — живая локальная структура нужного типа; `clock_gettime` пишет только в неё.
        let status = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut spec) };
        if status != 0 {
            return None;
        }
        // (на aarch64 и x86_64 `time_t` и `tv_nsec` — это `i64`.) Секунды в наносекунды без переполнения: при переполнении честный отказ, а не мусор.
        let seconds_as_nanos = spec.tv_sec.checked_mul(1_000_000_000)?;
        let nanos = seconds_as_nanos.checked_add(spec.tv_nsec)?;
        Some(SensorTimestamp::from_nanos(nanos))
    }
}
