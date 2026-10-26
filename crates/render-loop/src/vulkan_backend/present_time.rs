//! Желаемое время показа кадра (`desiredPresentTime` из `VK_GOOGLE_display_timing`).
//!
//! Правило платформы: кадр показывается на первом vsync, который не раньше желаемого
//! времени. Choreographer отдаёт ожидаемое время показа ровно на границе vsync, и метка
//! дрожит на доли миллисекунды в обе стороны. Если поставить её как есть, часть кадров
//! получает метку «на волосок позже» своего vsync: SurfaceFlinger держит такой кадр до
//! следующего vsync (кадр на экране 2 периода), а потом выбрасывает догнавший его кадр.
//! Замер сессии 03 на OnePlus 11 (60 Гц, release):
//! * без упреждения — 4–5 % интервалов по 2 периода и ~5 % кадров без показа;
//! * с упреждением на полпериода — 1133 интервала из 1133 ровно по одному периоду, ни одного
//!   выброшенного кадра.
//!
//! Поэтому метку ставим за `lead_ratio` периода ДО ожидаемого vsync: в середину интервала
//! между предыдущим vsync и целевым. Дрожание меньше половины периода больше не меняет vsync.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

/// Как выбирать желаемое время показа.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PresentTimePolicy {
    /// Доля периода обновления, на которую метка ставится раньше ожидаемого vsync
    /// (`RenderLoopConfig::present_time_lead_ratio`, по умолчанию 0,5).
    pub(super) lead_ratio: f64,
    /// Период, если драйвер ещё не сообщил реальный (`refresh_cycle`): период желаемой частоты.
    pub(super) fallback_refresh_period: Duration,
}

impl PresentTimePolicy {
    /// Желаемое время показа кадра, который должен появиться на vsync `expected_present`.
    /// `measured_refresh_period` — реальный период экрана от драйвера, если известен.
    pub(super) fn desired_present_time(
        &self,
        expected_present: MonotonicNanos,
        measured_refresh_period: Option<Duration>,
    ) -> MonotonicNanos {
        let period = measured_refresh_period.unwrap_or(self.fallback_refresh_period);
        let lead_nanos = period.as_secs_f64() * self.lead_ratio * 1e9;
        // Период — миллисекунды, упреждение заведомо помещается в i64; округляем к ближайшей наносекунде.
        let lead = lead_nanos.round() as i64;
        MonotonicNanos::from_nanos(expected_present.as_nanos().saturating_sub(lead))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD_60_HZ: Duration = Duration::from_nanos(16_666_667);

    fn policy(lead_ratio: f64) -> PresentTimePolicy {
        PresentTimePolicy {
            lead_ratio,
            fallback_refresh_period: PERIOD_60_HZ,
        }
    }

    /// На каком vsync (номер в сетке с шагом `period` от нуля) платформа покажет кадр:
    /// на первом vsync, который не раньше желаемого времени.
    fn presenting_vsync(desired: MonotonicNanos, period: Duration) -> i64 {
        let period_nanos = i64::try_from(period.as_nanos()).unwrap();
        (desired.as_nanos() + period_nanos - 1).div_euclid(period_nanos)
    }

    /// Ожидаемый показ на vsync `target`, сдвинутый дрожанием Choreographer на `jitter_nanos`.
    fn jittered_expected(target: i64, jitter_nanos: i64) -> MonotonicNanos {
        MonotonicNanos::from_nanos(target * 16_666_667 + jitter_nanos)
    }

    #[test]
    fn default_lead_targets_the_middle_of_the_interval_before_the_vsync() {
        let desired = policy(0.5).desired_present_time(MonotonicNanos::from_nanos(100_000_000), None);
        assert_eq!(desired.as_nanos(), 100_000_000 - 8_333_334);
    }

    #[test]
    fn jitter_around_the_vsync_never_moves_the_frame_to_the_next_vsync() {
        // Дрожание ±1 мс (на телефоне — доли миллисекунды): кадр всегда на своём vsync.
        for jitter in [-1_000_000, -10_000, -1, 0, 1, 10_000, 1_000_000] {
            let desired = policy(0.5).desired_present_time(jittered_expected(600, jitter), Some(PERIOD_60_HZ));
            assert_eq!(presenting_vsync(desired, PERIOD_60_HZ), 600, "дрожание {jitter} нс");
        }
    }

    #[test]
    fn without_lead_a_tiny_late_jitter_pushes_the_frame_one_vsync_later() {
        // Это и был дефект: метка на 1 нс позже vsync даёт показ на следующем vsync.
        let desired = policy(0.0).desired_present_time(jittered_expected(600, 1), Some(PERIOD_60_HZ));
        assert_eq!(presenting_vsync(desired, PERIOD_60_HZ), 601);
    }

    #[test]
    fn measured_period_wins_over_the_fallback() {
        let period_120_hz = Duration::from_nanos(8_333_333);
        let expected = MonotonicNanos::from_nanos(50_000_000);
        let desired = policy(0.5).desired_present_time(expected, Some(period_120_hz));
        assert_eq!(desired.as_nanos(), 50_000_000 - 4_166_667);
    }
}
