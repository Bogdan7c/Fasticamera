//! Тряска отказа: затухающие колебания вбок; при Reduce Motion — затемнение-возврат без сдвига.

use std::f64::consts::{PI, TAU};

use ui_core::time::{MonotonicNanos, MotionPreference};

use super::elapsed_seconds;
use crate::tokens::AttentionTokens;

/// Тряска отказа. Владелец — экран: он зовёт [`RefusalShake::trigger`] в момент отказа и рисует со сдвигом
/// [`RefusalShake::offset_x_pt`] и прозрачностью [`RefusalShake::opacity`].
#[derive(Debug, Clone)]
pub struct RefusalShake {
    tokens: AttentionTokens,
    motion: MotionPreference,
    /// Когда запущена последняя тряска; `None` — не трясётся.
    started: Option<MonotonicNanos>,
}

impl RefusalShake {
    /// Покоящаяся тряска. Токены берутся копией при создании.
    pub fn new(tokens: &AttentionTokens, motion: MotionPreference) -> Self {
        Self {
            tokens: *tokens,
            motion,
            started: None,
        }
    }

    /// Сменить предпочтение движения на лету. Идущая тряска доигрывается по новым правилам со своего же момента.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.motion = motion;
    }

    /// Запустить тряску в `now`. Повторный запуск перезапускает её с начала.
    pub fn trigger(&mut self, now: MonotonicNanos) {
        self.started = Some(now);
    }

    /// Длительность текущего вида эффекта, с.
    fn duration_seconds(&self) -> f64 {
        match self.motion {
            MotionPreference::Full => self.tokens.shake_duration_s,
            MotionPreference::Reduced => self.tokens.reduced_dim_duration_s,
        }
    }

    /// Доля пройденного эффекта в `now`, 0..1; `None` — эффект не идёт (не запущен или уже закончился).
    fn progress(&self, now: MonotonicNanos) -> Option<f64> {
        let elapsed = elapsed_seconds(self.started?, now);
        let duration = self.duration_seconds();
        (elapsed < duration).then_some(elapsed / duration)
    }

    /// Нужно ли ещё рисовать кадры.
    pub fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.progress(now).is_some()
    }

    /// Сдвиг вбок, pt: затухающая синусоида (вправо первой). При Reduce Motion — всегда 0.
    pub fn offset_x_pt(&self, now: MonotonicNanos) -> f64 {
        if self.motion == MotionPreference::Reduced {
            return 0.0;
        }
        match self.progress(now) {
            Some(progress) => {
                let envelope = 1.0 - progress;
                self.tokens.shake_amplitude_pt * envelope * (TAU * self.tokens.shake_periods * progress).sin()
            }
            None => 0.0,
        }
    }

    /// Прозрачность, 0..=1: при полном движении всегда 1; при Reduce Motion — затемнение до
    /// `reduced_dim_opacity` и плавный возврат.
    pub fn opacity(&self, now: MonotonicNanos) -> f64 {
        if self.motion == MotionPreference::Full {
            return 1.0;
        }
        match self.progress(now) {
            Some(progress) => {
                let dip = (PI * progress).sin();
                1.0 - (1.0 - self.tokens.reduced_dim_opacity) * dip
            }
            None => 1.0,
        }
    }
}
