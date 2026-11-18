//! Ограничитель частоты публикаций предложения AE.
//!
//! Каждое предложение заставляет движок камеры пересобрать запрос HAL, поэтому публикуем не
//! чаще `publish_min_interval` (по меткам сенсора) и только если что-то изменилось: цель сдвинулась
//! не меньше чем на `publish_min_change_ev`, или сменились `convergence` / `quality`. Первое
//! предложение потока и смена области (новый тап) уходят сразу.

use std::time::Duration;

use camera_contract::auto_exposure::{MeasurementQuality, MeteredArea, ProposalConvergence};
use camera_contract::capture_result::SensorTimestamp;

use super::config::ExposureControllerConfig;

/// Что собираемся опубликовать.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ProposalCandidate {
    pub(super) target_log2: f64,
    pub(super) convergence: ProposalConvergence,
    pub(super) quality: MeasurementQuality,
    pub(super) area: MeteredArea,
}

/// Последнее опубликованное и момент публикации.
#[derive(Debug, Clone, Copy)]
struct Published {
    at: SensorTimestamp,
    candidate: ProposalCandidate,
}

/// Решает, публиковать ли кандидата сейчас, и помнит последнюю публикацию.
#[derive(Debug, Clone)]
pub(super) struct ProposalPublisher {
    min_interval: Duration,
    min_change_ev: f64,
    last: Option<Published>,
}

impl ProposalPublisher {
    pub(super) fn new(config: &ExposureControllerConfig) -> Self {
        Self {
            min_interval: Duration::from_millis(u64::from(config.publish_min_interval_ms)),
            min_change_ev: f64::from(config.publish_min_change_ev),
            last: None,
        }
    }

    /// Забыть прошлые публикации (новый поток: первое предложение уйдёт сразу).
    pub(super) fn reset(&mut self) {
        self.last = None;
    }

    /// Публиковать ли `candidate` на кадре с меткой `now`. При «да» публикация записывается.
    pub(super) fn try_publish(&mut self, now: SensorTimestamp, candidate: ProposalCandidate) -> bool {
        let allowed = match self.last {
            None => true,
            Some(previous) if previous.candidate.area != candidate.area => true,
            Some(previous) => {
                // Метки не по порядку дают «интервал не прошёл»: публикация подождёт нормального кадра.
                let interval_elapsed = now
                    .duration_since(previous.at)
                    .is_some_and(|elapsed| elapsed >= self.min_interval);
                let state_changed = previous.candidate.convergence != candidate.convergence
                    || previous.candidate.quality != candidate.quality;
                let moved = (candidate.target_log2 - previous.candidate.target_log2).abs() >= self.min_change_ev;
                interval_elapsed && (state_changed || moved)
            }
        };
        if allowed {
            self.last = Some(Published { at: now, candidate });
        }
        allowed
    }
}
