//! Время интерфейса и часы анимаций.
//!
//! Главное правило `ui-core`: **никаких `Instant::now()` внутри**. Время приходит снаружи:
//! * касания несут свою метку `MotionEvent` (`CLOCK_MONOTONIC`);
//! * кадр несёт время vsync из рендера ([`shell_contract::vsync::VsyncTick::frame_time`], та же шкала).
//!
//! Поэтому один и тот же поток касаний и кадров даёт побитово одинаковый результат — это и делает
//! механику проверяемой формулами на ПК.
//!
//! Время хранится целыми наносекундами ([`MonotonicNanos`]). Физика берёт разность «сейчас − старт»
//! и переводит её в секунды `f64` один раз ([`UiDuration::as_secs_f64`]); шаги `dt` не накапливаются,
//! поэтому выборки на 60 и 120 Гц совпадают в общих точках.

use serde::{Deserialize, Serialize};
pub use shell_contract::clock::MonotonicNanos;

/// Длительность в наносекундах. Может быть отрицательной только как результат [`UiDuration::between`],
/// когда время пошло назад; потребители обязаны это различать (см. [`AnimationClock`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UiDuration(i64);

/// Наносекунд в секунде.
const NANOS_PER_SECOND: f64 = 1_000_000_000.0;
/// Наносекунд в миллисекунде.
const NANOS_PER_MILLI: i64 = 1_000_000;

impl UiDuration {
    pub const ZERO: UiDuration = UiDuration(0);

    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub const fn from_millis(millis: i64) -> Self {
        Self(millis.saturating_mul(NANOS_PER_MILLI))
    }

    /// Длительность из секунд. Дробная часть округляется до ближайшей наносекунды, не-конечное
    /// значение даёт ноль — пороги конфига проверяются `validate()` раньше, чем попадают сюда.
    pub fn from_secs_f64(seconds: f64) -> Self {
        if !seconds.is_finite() {
            return Self::ZERO;
        }
        // `as` насыщается на границах i64 — для длительностей интерфейса это недостижимо, но не UB.
        Self((seconds * NANOS_PER_SECOND).round() as i64)
    }

    /// Разность `later − earlier`. Отрицательна, если `later` раньше `earlier`.
    pub const fn between(earlier: MonotonicNanos, later: MonotonicNanos) -> Self {
        Self(later.as_nanos().saturating_sub(earlier.as_nanos()))
    }

    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// Секунды для формул физики.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / NANOS_PER_SECOND
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }
}

/// Момент `base + duration` (насыщение на границах шкалы).
pub const fn add_duration(base: MonotonicNanos, duration: UiDuration) -> MonotonicNanos {
    MonotonicNanos::from_nanos(base.as_nanos().saturating_add(duration.as_nanos()))
}

/// Как показывать движение: задел под системную «Удалить анимации» / Reduce Motion
/// (`docs/ios-camera/motion.md` раздел 3, `feature-map.md` строка про Reduce Motion).
///
/// В v1 источник флага — конфиг экрана; читать системную настройку будет сессия 16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionPreference {
    /// Обычные пружины и прокрутка.
    #[default]
    Full,
    /// Без упругости: пружины не перелетают (элементы `ui-kit` выбирают вариант без `bounce`).
    Reduced,
}

/// Итог продвижения часов на новый кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockAdvance {
    /// Время ушло вперёд.
    Advanced { elapsed: UiDuration },
    /// Пришёл тот же момент, что уже был (повтор кадра) — время не меняется.
    Repeated,
    /// Пришёл момент раньше текущего (сбой источника). Часы **не** идут назад: анимации
    /// останутся в текущей точке, событие видно в счётчике [`AnimationClock::backwards_ticks`].
    IgnoredBackwards { by: UiDuration },
}

/// Часы анимаций экрана: текущее время кадра и предпочтение движения.
///
/// Владелец — экран (`camera-screen`); `ui-core` только задаёт правила: время монотонно и
/// приходит снаружи. До первого кадра часов нет ([`AnimationClock::now`] = `None`).
#[derive(Debug, Clone, Default)]
pub struct AnimationClock {
    now: Option<MonotonicNanos>,
    motion: MotionPreference,
    backwards_ticks: u64,
}

impl AnimationClock {
    pub fn new(motion: MotionPreference) -> Self {
        Self {
            now: None,
            motion,
            backwards_ticks: 0,
        }
    }

    /// Продвинуть часы ко времени нового кадра (vsync из рендера).
    pub fn advance_to(&mut self, frame_time: MonotonicNanos) -> ClockAdvance {
        match self.now {
            None => {
                self.now = Some(frame_time);
                ClockAdvance::Advanced {
                    elapsed: UiDuration::ZERO,
                }
            }
            Some(current) if frame_time > current => {
                self.now = Some(frame_time);
                ClockAdvance::Advanced {
                    elapsed: UiDuration::between(current, frame_time),
                }
            }
            Some(current) if frame_time == current => ClockAdvance::Repeated,
            Some(current) => {
                self.backwards_ticks = self.backwards_ticks.saturating_add(1);
                ClockAdvance::IgnoredBackwards {
                    by: UiDuration::between(frame_time, current),
                }
            }
        }
    }

    /// Время текущего кадра; `None` до первого кадра.
    pub fn now(&self) -> Option<MonotonicNanos> {
        self.now
    }

    pub fn motion(&self) -> MotionPreference {
        self.motion
    }

    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.motion = motion;
    }

    /// Сколько раз источник прислал время раньше текущего.
    pub fn backwards_ticks(&self) -> u64 {
        self.backwards_ticks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(nanos: i64) -> MonotonicNanos {
        MonotonicNanos::from_nanos(nanos)
    }

    #[test]
    fn durations_convert_exactly() {
        assert_eq!(UiDuration::from_millis(500).as_nanos(), 500_000_000);
        assert_eq!(UiDuration::from_secs_f64(0.5), UiDuration::from_millis(500));
        assert_eq!(UiDuration::from_secs_f64(f64::NAN), UiDuration::ZERO);
        assert_eq!(UiDuration::between(at(10), at(4)).as_nanos(), -6);
        assert!((UiDuration::from_millis(250).as_secs_f64() - 0.25).abs() < 1e-15);
        assert_eq!(add_duration(at(1_000), UiDuration::from_nanos(500)), at(1_500));
    }

    #[test]
    fn clock_is_monotonic_and_counts_backwards_ticks() {
        let mut clock = AnimationClock::new(MotionPreference::Full);
        assert_eq!(clock.now(), None);
        assert_eq!(
            clock.advance_to(at(100)),
            ClockAdvance::Advanced {
                elapsed: UiDuration::ZERO
            }
        );
        assert_eq!(
            clock.advance_to(at(150)),
            ClockAdvance::Advanced {
                elapsed: UiDuration::from_nanos(50)
            }
        );
        assert_eq!(clock.advance_to(at(150)), ClockAdvance::Repeated);
        assert_eq!(
            clock.advance_to(at(120)),
            ClockAdvance::IgnoredBackwards {
                by: UiDuration::from_nanos(30)
            }
        );
        assert_eq!(clock.now(), Some(at(150)), "время назад не идёт");
        assert_eq!(clock.backwards_ticks(), 1);
    }

    #[test]
    fn motion_preference_is_kept_and_switchable() {
        let mut clock = AnimationClock::new(MotionPreference::Reduced);
        assert_eq!(clock.motion(), MotionPreference::Reduced);
        clock.set_motion(MotionPreference::Full);
        assert_eq!(clock.motion(), MotionPreference::Full);
    }
}
