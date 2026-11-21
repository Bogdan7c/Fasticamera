//! Делёж «выдержка × ISO»: как набрать нужное произведение в пределах камеры.
//!
//! Чистая функция без обращений к состоянию: вход — нужное произведение, что закреплено
//! пользователем, пределы и период мерцания; выход — выдержка и ISO. Правила (решение 08a):
//! * сначала тянем выдержку (меньше шума), ISO — остаток;
//! * при защите от мерцания выдержка, если она не короче периода `p`, округляется **вниз** до
//!   кратной `p`, а недостачу доплачивает ISO;
//! * предел выдержки — меньшее из «самая длинная у камеры» и «длительность кадра потока»;
//! * что не влезло в пределы — не скрываем: считает вызывающий по результату (нехватка в EV).

use std::time::Duration;

/// Пределы, внутри которых делим.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplitLimits {
    pub(crate) iso_min: u32,
    pub(crate) iso_max: u32,
    pub(crate) shutter_shortest: Duration,
    /// Самая длинная допустимая выдержка: `min(у камеры, длительность кадра потока)`.
    pub(crate) shutter_cap: Duration,
}

/// Что закреплено пользователем (уже приведённое к пределам камеры).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    /// Ничего: делим свободно.
    Free,
    /// ISO закреплён, выдержку подбираем.
    Iso(u32),
    /// Выдержка закреплена, подбираем ISO.
    Shutter(Duration),
}

/// Итог дележа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Split {
    pub(crate) shutter: Duration,
    pub(crate) iso: u32,
}

impl Split {
    /// Произведение «выдержка × ISO», с·ISO.
    pub(crate) fn product_seconds_iso(self) -> f64 {
        self.shutter.as_secs_f64() * f64::from(self.iso)
    }
}

/// Разделить `requested_seconds_iso` (> 0, конечное) на выдержку и ISO.
pub(crate) fn split(
    requested_seconds_iso: f64,
    anchor: Anchor,
    limits: SplitLimits,
    flicker_period: Option<Duration>,
) -> Split {
    match anchor {
        Anchor::Free => {
            // Самая длинная выдержка при самом низком ISO — «идеал» по шуму.
            let wanted = seconds_to_duration(requested_seconds_iso / f64::from(limits.iso_min.max(1)));
            let shutter = shutter_for(wanted, limits, flicker_period, PeriodRounding::Down);
            Split {
                shutter,
                iso: iso_for(requested_seconds_iso, shutter, limits),
            }
        }
        Anchor::Iso(iso) => {
            let iso = iso.clamp(limits.iso_min, limits.iso_max);
            let wanted = seconds_to_duration(requested_seconds_iso / f64::from(iso.max(1)));
            Split {
                // ISO закреплён и не может доплатить: ближайшая кратная даёт наименьшую ошибку.
                shutter: shutter_for(wanted, limits, flicker_period, PeriodRounding::Nearest),
                iso,
            }
        }
        Anchor::Shutter(shutter) => Split {
            shutter,
            iso: iso_for(requested_seconds_iso, shutter, limits),
        },
    }
}

/// Как округлять выдержку до кратной периоду мерцания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeriodRounding {
    /// Вниз: ISO потом доплатит недостачу (свободный делёж).
    Down,
    /// К ближайшей (но не выше предела): доплатить некому, ошибка должна быть минимальной.
    Nearest,
}

/// Выдержка для желаемой `wanted`: кратность периоду мерцания либо просто пределы.
fn shutter_for(
    wanted: Duration,
    limits: SplitLimits,
    flicker_period: Option<Duration>,
    rounding: PeriodRounding,
) -> Duration {
    match flicker_period {
        // Кратность возможна, только если период влезает и в желаемое, и в предел.
        Some(period) if !period.is_zero() && wanted >= period && limits.shutter_cap >= period => {
            let usable = wanted.min(limits.shutter_cap);
            let floor_periods = usable.as_nanos() / period.as_nanos();
            let nearest_periods = (wanted.as_nanos() + period.as_nanos() / 2) / period.as_nanos();
            // Ближайшая кратная выше предела кадра недопустима — тогда вниз от предела.
            let whole_periods = match rounding {
                PeriodRounding::Down => floor_periods,
                PeriodRounding::Nearest if nearest_periods * period.as_nanos() <= limits.shutter_cap.as_nanos() => {
                    nearest_periods
                }
                PeriodRounding::Nearest => floor_periods,
            };
            // `whole_periods >= 1`, потому что `usable >= period`; в `u32` влезает с запасом
            // (период — не меньше миллисекунды, предел выдержки — секунды).
            period * u32::try_from(whole_periods).unwrap_or(u32::MAX)
        }
        _ => wanted.clamp(limits.shutter_shortest, limits.shutter_cap.max(limits.shutter_shortest)),
    }
}

/// ISO, доплачивающий недостающее произведение при уже выбранной выдержке.
fn iso_for(requested_seconds_iso: f64, shutter: Duration, limits: SplitLimits) -> u32 {
    let exact = requested_seconds_iso / shutter.as_secs_f64().max(f64::MIN_POSITIVE);
    // `as` насыщает: слишком большое число станет `u32::MAX`, NaN — 0; `clamp` вернёт в границы.
    let rounded = exact.round() as u32;
    rounded.clamp(limits.iso_min, limits.iso_max)
}

/// Секунды → `Duration` без паники на бесконечности и мусоре.
fn seconds_to_duration(seconds: f64) -> Duration {
    Duration::try_from_secs_f64(seconds).unwrap_or(if seconds > 0.0 { Duration::MAX } else { Duration::ZERO })
}
