//! Механика плашки: показать, скрыть, срок жизни, снимок кадра, события.

use ui_core::time::{MonotonicNanos, MotionPreference, UiDuration, add_duration};

use super::error::BannerError;
use crate::environment::ElementEnvironment;
use crate::picker::LabelId;
use crate::progress::{SpringProgress, SpringSource};
use crate::tokens::MotionToken;

/// Сколько живёт плашка после показа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerLifetime {
    /// Пока вызывающий не скажет скрыть (AE/AF LOCK держится, пока блокировка не снята).
    UntilHidden,
    /// Столько времени с момента показа, затем скрывается сама.
    For(UiDuration),
}

/// Фаза плашки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerPhase {
    Hidden,
    /// Появляется.
    Showing,
    /// Видна полностью.
    Visible,
    /// Скрывается.
    Hiding,
}

/// Почему плашка скрылась.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideCause {
    /// Вызывающий попросил.
    Requested,
    /// Вышел срок [`BannerLifetime::For`].
    Expired,
}

/// Итог `show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerShowOutcome {
    /// Плашка показана из скрытого состояния.
    Started,
    /// Плашка скрывалась и развернута обратно (скорость унаследована).
    Reversed,
    /// Плашка уже была показана: ключ текста и срок обновлены, анимации нет.
    Refreshed,
}

/// Итог `hide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerHideOutcome {
    Started,
    /// Плашка появлялась и развернута в скрытие.
    Reversed,
    /// Плашка уже скрыта или скрывается.
    AlreadyThere,
}

/// Событие плашки; по одному на завершённое движение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerEvent {
    /// Плашка полностью появилась.
    Shown { label: LabelId },
    /// Плашка полностью скрылась.
    Hidden { label: LabelId, cause: HideCause },
}

/// Срез состояния плашки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BannerSnapshot {
    pub phase: BannerPhase,
    /// Ключ текста; `None`, когда плашка полностью скрыта и рисовать нечего.
    pub label: Option<LabelId>,
    /// Прозрачность 0..1.
    pub opacity: f32,
    /// Масштаб: растёт от `appear_scale` к 1 при появлении.
    pub scale: f64,
}

/// Куда плашка едет: внутреннее направление движения вместо позиционного `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// К полной видимости.
    Show,
    /// К скрытому состоянию.
    Hide,
}

/// Плашка.
#[derive(Debug, Clone)]
pub struct Banner {
    springs: SpringSource,
    appear_scale: f64,
    /// 0 — скрыта, 1 — видна.
    progress: SpringProgress,
    wants_visible: bool,
    label: Option<LabelId>,
    /// Момент, когда плашка должна начать скрываться сама.
    expires_at: Option<MonotonicNanos>,
    hide_cause: HideCause,
    unreported_settle: bool,
    events: Vec<BannerEvent>,
}

impl Banner {
    /// Пружина появления и скрытия: `.smooth` без перелёта (`motion.md` 4.2).
    const SPRING: MotionToken = MotionToken::Smooth;

    /// Создать скрытую плашку.
    pub fn new(environment: &ElementEnvironment<'_>, now: MonotonicNanos) -> Result<Self, BannerError> {
        environment.tokens.validate()?;
        let springs = SpringSource::new(&environment.tokens.motion, environment.motion);
        let progress = SpringProgress::at_rest(0.0, springs.spring(Self::SPRING)?, now)?;
        Ok(Self {
            springs,
            appear_scale: environment.tokens.banner.appear_scale,
            progress,
            wants_visible: false,
            label: None,
            expires_at: None,
            hide_cause: HideCause::Requested,
            unreported_settle: false,
            events: Vec::new(),
        })
    }

    /// Показать плашку с текстом `label`. Срок отсчитывается от `now`.
    pub fn show(
        &mut self,
        label: LabelId,
        lifetime: BannerLifetime,
        now: MonotonicNanos,
    ) -> Result<BannerShowOutcome, BannerError> {
        let expires_at = match lifetime {
            BannerLifetime::UntilHidden => None,
            BannerLifetime::For(duration) if duration > UiDuration::ZERO => Some(add_duration(now, duration)),
            BannerLifetime::For(_) => return Err(BannerError::NonPositiveLifetime),
        };
        // Прежде чем менять подпись и цель, фиксируем всё, что к `now` уже случилось: полностью
        // скрывшаяся (или истёкшая) плашка обязана выдать `Hidden` со СВОЕЙ подписью и причиной,
        // иначе новый `show` затёр бы их (AE/AF LOCK снят и тут же поставлен снова).
        self.advance(now)?;
        let was_wanted = self.wants_visible;
        let was_moving = self.progress.is_moving(now);
        self.label = Some(label);
        self.expires_at = expires_at;
        if was_wanted {
            return Ok(BannerShowOutcome::Refreshed);
        }
        self.retarget(Direction::Show, now)?;
        Ok(if was_moving {
            BannerShowOutcome::Reversed
        } else {
            BannerShowOutcome::Started
        })
    }

    /// Скрыть плашку по просьбе вызывающего.
    pub fn hide(&mut self, now: MonotonicNanos) -> Result<BannerHideOutcome, BannerError> {
        // Срок, истёкший до `now`, но ещё не замеченный кадром, скрывает плашку с причиной `Expired`.
        self.advance(now)?;
        if !self.wants_visible {
            return Ok(BannerHideOutcome::AlreadyThere);
        }
        let was_moving = self.progress.is_moving(now);
        self.begin_hide(HideCause::Requested, now)?;
        Ok(if was_moving {
            BannerHideOutcome::Reversed
        } else {
            BannerHideOutcome::Started
        })
    }

    fn begin_hide(&mut self, cause: HideCause, at: MonotonicNanos) -> Result<(), BannerError> {
        self.expires_at = None;
        self.hide_cause = cause;
        self.retarget(Direction::Hide, at)
    }

    fn retarget(&mut self, direction: Direction, at: MonotonicNanos) -> Result<(), BannerError> {
        let spring = self.springs.spring(Self::SPRING)?;
        let (target, wants_visible) = match direction {
            Direction::Show => (1.0, true),
            Direction::Hide => (0.0, false),
        };
        self.progress.retarget(spring, target, at)?;
        self.wants_visible = wants_visible;
        self.unreported_settle = true;
        Ok(())
    }

    /// Reduce Motion включили или выключили на лету.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.springs.set_motion(motion);
    }

    /// Фаза в момент `now`.
    pub fn phase(&self, now: MonotonicNanos) -> BannerPhase {
        match (self.progress.is_moving(now), self.wants_visible) {
            (true, true) => BannerPhase::Showing,
            (true, false) => BannerPhase::Hiding,
            (false, true) => BannerPhase::Visible,
            (false, false) => BannerPhase::Hidden,
        }
    }

    /// Раз в кадр: истёкший срок начинает скрытие, завершённое движение даёт событие (один раз).
    pub fn advance(&mut self, now: MonotonicNanos) -> Result<(), BannerError> {
        if let Some(deadline) = self.expires_at
            && self.wants_visible
            && deadline <= now
        {
            // Скрытие стартует в момент истечения, а не в момент кадра: положение не зависит от частоты кадров.
            self.begin_hide(HideCause::Expired, deadline)?;
        }
        if self.progress.is_moving(now) || !self.unreported_settle {
            return Ok(());
        }
        self.unreported_settle = false;
        if let Some(label) = self.label {
            if self.wants_visible {
                self.events.push(BannerEvent::Shown { label });
            } else {
                self.events.push(BannerEvent::Hidden {
                    label,
                    cause: self.hide_cause,
                });
                self.label = None;
            }
        }
        Ok(())
    }

    /// Забрать накопленные события (очередь очищается).
    pub fn drain_events(&mut self) -> Vec<BannerEvent> {
        std::mem::take(&mut self.events)
    }

    /// Всё для рисования за один вызов.
    pub fn snapshot(&self, now: MonotonicNanos) -> BannerSnapshot {
        let progress = self.progress.value(now);
        let phase = self.phase(now);
        BannerSnapshot {
            phase,
            label: if phase == BannerPhase::Hidden && !self.wants_visible {
                None
            } else {
                self.label
            },
            opacity: progress.clamp(0.0, 1.0) as f32,
            scale: self.appear_scale + (1.0 - self.appear_scale) * progress,
        }
    }
}
