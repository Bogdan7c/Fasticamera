//! Появление рамки по времени: линейное сжатие и линейное проявление за фиксированный срок.
//!
//! Пружина для этого не годится: в записи (`[measured: R00 t=99.05–99.17]`) рамка сжимается с 1,7×
//! до 1,0× ровно за 0,12 с, прозрачность растёт линейно, перелёта нет. Кривая детерминирована:
//! значение зависит только от `now - started_at`, а не от частоты кадров.

use ui_core::time::{MonotonicNanos, MotionPreference, UiDuration};

/// Таймерная кривая появления.
#[derive(Debug, Clone, Copy)]
pub(super) struct TimedAppearance {
    /// Когда началось последнее появление; `None` — рамку ещё не ставили (считается завершённым).
    started_at: Option<MonotonicNanos>,
    duration_s: f64,
    from_scale: f64,
    /// Reduce Motion: без масштабирования, только проявление (масштаб — это движение).
    motion: MotionPreference,
}

impl TimedAppearance {
    pub(super) fn new(from_scale: f64, duration_s: f64, motion: MotionPreference) -> Self {
        Self {
            started_at: None,
            duration_s,
            from_scale,
            motion,
        }
    }

    /// Reduce Motion включили или выключили на лету: действует со следующего кадра.
    pub(super) fn set_motion(&mut self, motion: MotionPreference) {
        self.motion = motion;
    }

    /// Начать появление заново с момента `now`.
    pub(super) fn restart(&mut self, now: MonotonicNanos) {
        self.started_at = Some(now);
    }

    /// Доля пройденного пути 0..1 (линейная).
    fn fraction(&self, now: MonotonicNanos) -> f64 {
        let Some(started_at) = self.started_at else {
            return 1.0;
        };
        let elapsed_s = UiDuration::between(started_at, now).as_secs_f64();
        (elapsed_s / self.duration_s).clamp(0.0, 1.0)
    }

    /// Масштаб: от `from_scale` к 1 линейно. При Reduce Motion рамка не «падает» с увеличения — сразу 1,0,
    /// появление идёт одним проявлением (`opacity`). Дизайн-правило проекта, `[estimate]`: у Apple не снято.
    pub(super) fn scale(&self, now: MonotonicNanos) -> f64 {
        if self.motion == MotionPreference::Reduced {
            return 1.0;
        }
        self.from_scale + (1.0 - self.from_scale) * self.fraction(now)
    }

    /// Прозрачность появления 0..1 (линейная); умножается на прозрачность удержания.
    pub(super) fn opacity(&self, now: MonotonicNanos) -> f32 {
        self.fraction(now) as f32
    }
}
