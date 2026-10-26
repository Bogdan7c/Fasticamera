//! Схождение частоты экрана для сценария `render`: чистая логика без ввода-вывода.
//!
//! Мгновенного совпадения периода с целевым мало: при подключении драйвер может на миг
//! отдать «переходное» значение (OxygenOS сначала сообщает 8,33 мс, через ~250 мс применяет
//! колпак 60 Гц). Поэтому экран считается сошедшимся, только когда И период экрана, И период,
//! по которому судит пейсинг, непрерывно держатся в допуске не меньше `stable_for`.
//! Любой выход из допуска обнуляет отсчёт.

use std::time::{Duration, Instant};

use render_loop::thread::RenderStatus;

use super::RenderCheckParams;

/// Как наблюдаемый период соотносится с целевым (для стадии `awaiting_refresh` и причины отказа).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshShortfall {
    /// Период экрана неизвестен (нет `display_timing`).
    Unknown,
    /// Экран МЕДЛЕННЕЕ цели (период длиннее, например колпак 60 Гц при цели 120). Только в этом
    /// случае касание имеет смысл: оно снимает колпак OxygenOS.
    DisplaySlower,
    /// Экран БЫСТРЕЕ цели (период короче). Касание не поможет — это отдельный случай.
    DisplayFaster,
    /// Экран в допуске, но пейсинг ещё судит по другому периоду либо не прошло `stable_for`.
    NotYetStable,
}

/// Решение трекера на очередном статусе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettleDecision {
    /// Период непрерывно в допуске не меньше `stable_for`.
    Settled,
    /// Ещё нет; `observed` — период экрана (`None` — неизвестен).
    Waiting {
        observed: Option<Duration>,
        shortfall: RefreshShortfall,
    },
}

/// Отсчитывает непрерывное пребывание периода в допуске.
#[derive(Debug, Default)]
pub struct RefreshSettleTracker {
    /// С какого момента период непрерывно в допуске (`None` — сейчас вне допуска).
    on_target_since: Option<Instant>,
}

impl RefreshSettleTracker {
    /// Учитывает статус, снятый в момент `now`. `stable_for` — сколько обязан держаться допуск.
    pub fn update(
        &mut self,
        now: Instant,
        status: &RenderStatus,
        params: &RenderCheckParams,
        stable_for: Duration,
    ) -> SettleDecision {
        let observed = status.display.as_ref().and_then(|info| info.refresh_cycle);
        let on_target = |period: Duration| {
            super::relative_deviation(period, params.expected_period) <= params.refresh_tolerance_ratio
        };
        let both_on_target = observed.is_some_and(on_target) && on_target(status.pacing_refresh_period);
        if !both_on_target {
            self.on_target_since = None;
            return SettleDecision::Waiting {
                observed,
                shortfall: classify_shortfall(observed, params),
            };
        }
        let since = *self.on_target_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= stable_for {
            SettleDecision::Settled
        } else {
            SettleDecision::Waiting {
                observed,
                shortfall: RefreshShortfall::NotYetStable,
            }
        }
    }
}

/// Куда смотрит отклонение периода экрана от цели (когда он вне допуска или пейсинг отстаёт).
fn classify_shortfall(observed: Option<Duration>, params: &RenderCheckParams) -> RefreshShortfall {
    let Some(period) = observed else {
        return RefreshShortfall::Unknown;
    };
    if super::relative_deviation(period, params.expected_period) <= params.refresh_tolerance_ratio {
        // Экран уже в допуске, вне допуска только период пейсинга.
        RefreshShortfall::NotYetStable
    } else if period > params.expected_period {
        RefreshShortfall::DisplaySlower
    } else {
        RefreshShortfall::DisplayFaster
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test_support::presenting_status;

    const STABLE: Duration = Duration::from_millis(1500);

    fn params_120() -> RenderCheckParams {
        RenderCheckParams {
            expected_period: Duration::from_secs_f64(1.0 / 120.0),
            refresh_tolerance_ratio: 0.05,
            max_miss_ratio: 0.005,
            refresh_settled_after: Duration::ZERO,
            min_coverage_ratio: 0.9,
            synthetic_touch_requested: false,
        }
    }

    fn status_with_period(period: Duration) -> RenderStatus {
        let mut status = presenting_status();
        status.pacing_refresh_period = period;
        status.display.as_mut().expect("display").refresh_cycle = Some(period);
        status
    }

    #[test]
    fn transient_target_value_does_not_settle_until_it_is_held_continuously() {
        let target = Duration::from_secs_f64(1.0 / 120.0);
        let sixty = Duration::from_secs_f64(1.0 / 60.0);
        let params = params_120();
        let mut tracker = RefreshSettleTracker::default();
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);

        // Переходное 8,33 мс на старте, потом колпак 60 Гц, потом снова цель.
        assert!(matches!(
            tracker.update(at(0), &status_with_period(target), &params, STABLE),
            SettleDecision::Waiting {
                shortfall: RefreshShortfall::NotYetStable,
                ..
            }
        ));
        assert!(matches!(
            tracker.update(at(300), &status_with_period(sixty), &params, STABLE),
            SettleDecision::Waiting {
                shortfall: RefreshShortfall::DisplaySlower,
                ..
            }
        ));
        // Цель вернулась в момент 1000: отсчёт идёт с него, а не с нулевого переходного.
        assert!(matches!(
            tracker.update(at(1000), &status_with_period(target), &params, STABLE),
            SettleDecision::Waiting {
                shortfall: RefreshShortfall::NotYetStable,
                ..
            }
        ));
        assert!(
            matches!(
                tracker.update(at(2400), &status_with_period(target), &params, STABLE),
                SettleDecision::Waiting { .. }
            ),
            "прошло только 1400 мс непрерывной стабильности"
        );
        assert_eq!(
            tracker.update(at(2500), &status_with_period(target), &params, STABLE),
            SettleDecision::Settled
        );
    }

    #[test]
    fn falling_out_of_tolerance_resets_the_stability_countdown() {
        let target = Duration::from_secs_f64(1.0 / 120.0);
        let params = params_120();
        let mut tracker = RefreshSettleTracker::default();
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);

        tracker.update(at(0), &status_with_period(target), &params, STABLE);
        tracker.update(
            at(1000),
            &status_with_period(Duration::from_secs_f64(1.0 / 60.0)),
            &params,
            STABLE,
        );
        assert!(
            matches!(
                tracker.update(at(1600), &status_with_period(target), &params, STABLE),
                SettleDecision::Waiting { .. }
            ),
            "без сброса к 1600 мс было бы уже 1600 мс «стабильности»"
        );
    }

    #[test]
    fn shortfall_distinguishes_slower_faster_unknown_and_lagging_pacing() {
        let params = params_120();
        let mut tracker = RefreshSettleTracker::default();
        let now = Instant::now();
        let mut shortfall_of = |status: &RenderStatus| match tracker.update(now, status, &params, STABLE) {
            SettleDecision::Waiting { shortfall, .. } => shortfall,
            SettleDecision::Settled => panic!("не должно сходиться"),
        };

        let slower = status_with_period(Duration::from_secs_f64(1.0 / 60.0));
        assert_eq!(shortfall_of(&slower), RefreshShortfall::DisplaySlower);
        let faster = status_with_period(Duration::from_secs_f64(1.0 / 240.0));
        assert_eq!(shortfall_of(&faster), RefreshShortfall::DisplayFaster);
        let mut unknown = presenting_status();
        unknown.display.as_mut().expect("display").refresh_cycle = None;
        assert_eq!(shortfall_of(&unknown), RefreshShortfall::Unknown);
        // Экран на цели, пейсинг отстал: касание не поможет.
        let mut lagging = presenting_status();
        lagging.pacing_refresh_period = Duration::from_secs_f64(1.0 / 60.0);
        assert_eq!(shortfall_of(&lagging), RefreshShortfall::NotYetStable);
    }
}
