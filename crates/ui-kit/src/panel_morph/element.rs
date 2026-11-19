//! Механика панели-морфа: фазы, прерывание, снимок кадра, события.

use ui_core::geometry::{Point, Rect, Size};
use ui_core::time::{MonotonicNanos, MotionPreference};

use super::error::PanelError;
use super::spec::PanelMorphSpec;
use crate::environment::ElementEnvironment;
use crate::progress::{SpringProgress, SpringSource, UNIT_REST};
use crate::tokens::PanelTokens;

/// Фаза панели в момент выборки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelPhase {
    Closed,
    Opening,
    Open,
    Closing,
}

/// Куда панель едет: внутреннее направление вместо позиционного `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Open,
    Close,
}

/// Что вышло из вызова `open`/`close`. Различие важно экрану: «ничего не произошло» не должно
/// запускать хаптику или перерисовку, а разворот в полёте — особый случай.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// Движение началось из покоя.
    Started,
    /// Идущее в другую сторону движение развёрнуто (скорость унаследована).
    Reversed,
    /// Панель уже в этом состоянии или едет в эту сторону: ничего не изменилось.
    AlreadyThere,
}

/// Событие панели; по одному на завершённое движение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    /// Панель полностью открылась.
    Opened,
    /// Панель полностью закрылась.
    Closed,
}

/// Срез состояния панели для рисования и проверок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelSnapshot {
    pub phase: PanelPhase,
    /// Прогресс открытия: 0 — закрыта, 1 — открыта (пружина без перелёта не выходит за 0..1).
    pub progress: f64,
    /// Рамка стекла между источником и целью, pt.
    pub frame: Rect,
    /// Радиус скругления: от «капсулы источника» к радиусу цели, pt.
    pub corner_radius_pt: f64,
    /// Материализация стекла 0..1 (Apple 219, 2:55): сила линзирования, не прозрачность.
    pub effect: f32,
    /// Прозрачность содержимого 0..1: пункты проявляются во второй части раскрытия.
    pub content_opacity: f32,
    /// Сдвиг содержимого вниз, pt: пункты «въезжают снизу» (`motion.md` 4.1); 0 у открытой панели.
    pub content_offset_pt: f64,
    /// Прозрачность источника 0..1: кнопка гаснет, пока панель вырастает из неё.
    pub source_opacity: f32,
}

/// Панель-морф: рамка источника → рамка цели и обратно.
#[derive(Debug, Clone)]
pub struct PanelMorph {
    spec: PanelMorphSpec,
    tokens: PanelTokens,
    springs: SpringSource,
    progress: SpringProgress,
    /// Куда панель едет или где стоит: `true` — к открытию.
    wants_open: bool,
    /// Движение, о завершении которого ещё не сообщено событием.
    unreported_settle: Option<PanelEvent>,
    events: Vec<PanelEvent>,
}

impl PanelMorph {
    /// Создать закрытую панель. `environment.viewport_extent_points` панели не нужен.
    pub fn new(
        spec: PanelMorphSpec,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, PanelError> {
        spec.validate()?;
        environment.tokens.validate()?;
        let springs = SpringSource::new(&environment.tokens.motion, environment.motion);
        let progress = SpringProgress::at_rest(0.0, springs.spring(spec.spring)?, now)?;
        Ok(Self {
            spec,
            tokens: environment.tokens.panel,
            springs,
            progress,
            wants_open: false,
            unreported_settle: None,
            events: Vec::new(),
        })
    }

    pub fn spec(&self) -> &PanelMorphSpec {
        &self.spec
    }

    /// Фаза в момент `now`.
    pub fn phase(&self, now: MonotonicNanos) -> PanelPhase {
        match (self.progress.is_moving(now), self.wants_open) {
            (true, true) => PanelPhase::Opening,
            (true, false) => PanelPhase::Closing,
            (false, true) => PanelPhase::Open,
            (false, false) => PanelPhase::Closed,
        }
    }

    /// Открыть панель.
    pub fn open(&mut self, now: MonotonicNanos) -> Result<TransitionOutcome, PanelError> {
        self.move_towards(Direction::Open, now)
    }

    /// Закрыть панель (в том числе во время открытия: скорость наследуется).
    pub fn close(&mut self, now: MonotonicNanos) -> Result<TransitionOutcome, PanelError> {
        self.move_towards(Direction::Close, now)
    }

    /// Общая часть `open`/`close`.
    ///
    /// Событие рассказывает только о реальном движении:
    /// * сначала фиксируем всё, что к `now` уже завершилось (`advance`), иначе новое движение
    ///   затёрло бы ещё не выданное событие прежнего;
    /// * если панель в `now` уже стоит у новой цели (`open` и тут же `close` в один момент — она
    ///   не сдвинулась ни на шаг), события о «завершённом» движении не будет.
    fn move_towards(&mut self, direction: Direction, now: MonotonicNanos) -> Result<TransitionOutcome, PanelError> {
        self.advance(now);
        let (target, wants_open, event) = match direction {
            Direction::Open => (1.0, true, PanelEvent::Opened),
            Direction::Close => (0.0, false, PanelEvent::Closed),
        };
        if self.wants_open == wants_open {
            return Ok(TransitionOutcome::AlreadyThere);
        }
        let was_moving = self.progress.is_moving(now);
        let already_at_target = (self.progress.value(now) - target).abs() <= UNIT_REST.absolute_position_floor;
        let spring = self.springs.spring(self.spec.spring)?;
        self.progress.retarget(spring, target, now)?;
        self.wants_open = wants_open;
        self.unreported_settle = if already_at_target { None } else { Some(event) };
        Ok(if was_moving {
            TransitionOutcome::Reversed
        } else {
            TransitionOutcome::Started
        })
    }

    /// Reduce Motion включили или выключили на лету: действует на следующие движения.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.springs.set_motion(motion);
    }

    /// Раз в кадр: сообщить о завершении движения (ровно один раз).
    pub fn advance(&mut self, now: MonotonicNanos) {
        if self.progress.is_moving(now) {
            return;
        }
        if let Some(event) = self.unreported_settle.take() {
            self.events.push(event);
        }
    }

    /// Забрать накопленные события (очередь очищается).
    pub fn drain_events(&mut self) -> Vec<PanelEvent> {
        std::mem::take(&mut self.events)
    }

    /// Всё для рисования за один вызов.
    pub fn snapshot(&self, now: MonotonicNanos) -> PanelSnapshot {
        let progress = self.progress.value(now);
        // Геометрия может чуть выйти за 0..1 у пружины с перелётом, а доли — нет.
        let unit = progress.clamp(0.0, 1.0);
        let geometric = progress.max(0.0);
        let source = self.spec.source;
        let target = self.spec.target;
        let origin = Point::new(
            lerp(source.origin.x, target.origin.x, geometric),
            lerp(source.origin.y, target.origin.y, geometric),
        );
        let size = Size::new(
            lerp(source.size.width, target.size.width, geometric),
            lerp(source.size.height, target.size.height, geometric),
        );
        let source_radius = source.size.width.min(source.size.height) * 0.5;
        let reveal = self.tokens.content_reveal_start;
        PanelSnapshot {
            phase: self.phase(now),
            progress,
            frame: Rect::new(origin, size),
            corner_radius_pt: lerp(source_radius, self.spec.target_corner_radius_pt, geometric),
            effect: unit as f32,
            content_opacity: ((progress - reveal) / (1.0 - reveal)).clamp(0.0, 1.0) as f32,
            content_offset_pt: self.tokens.content_slide_distance_pt * (1.0 - unit),
            source_opacity: (1.0 - unit) as f32,
        }
    }
}

fn lerp(from: f64, to: f64, fraction: f64) -> f64 {
    from + (to - from) * fraction
}
