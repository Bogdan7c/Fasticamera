//! Движок прокрутки одной оси.
//!
//! Владеет состоянием движения (покой / палец / замедление / пружина) и границами.
//! Снаружи — только намерения: `begin_drag`, `drag_to`, `release`, `cancel`, `scroll_to`.
//! `sample(now)` чистая: все переходы (бросок → край → возврат) спланированы заранее
//! аналитически, поэтому одинаковый поток вызовов даёт побитово одинаковый результат.
//!
//! Знаки: все величины — в единицах смещения контента (offset). Если палец тянет вниз, а offset
//! при этом уменьшается, знак меняет вызывающий код (экран), а не движок.

use super::config::{ResolvedScrollConfig, ScrollConfig};
use super::deceleration::{DecelerationEnd, DecelerationSegment};
use super::error::ScrollError;
use super::rubber::RubberBand;
use super::snap::{choose_snap_point, nearest_index};
use crate::spring::{Spring, SpringAnimation, SpringPhase, SpringStart};
use crate::time::{MonotonicNanos, MotionPreference};

/// Границы смещения и размер видимой области (нужен для «резинки»).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisGeometry {
    pub min_offset: f64,
    pub max_offset: f64,
    /// Размер области вдоль оси, pt: верхний предел растяжения «резинки».
    pub viewport_extent: f64,
}

impl AxisGeometry {
    fn validate(&self) -> Result<(), ScrollError> {
        let bounds_ok =
            self.min_offset.is_finite() && self.max_offset.is_finite() && self.min_offset <= self.max_offset;
        if !bounds_ok {
            return Err(ScrollError::InvalidBounds {
                min: self.min_offset,
                max: self.max_offset,
            });
        }
        if !self.viewport_extent.is_finite() || self.viewport_extent <= 0.0 {
            return Err(ScrollError::InvalidViewportExtent(self.viewport_extent));
        }
        Ok(())
    }
}

/// Что сейчас делает контент.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPhase {
    Resting,
    /// Контент ведёт палец.
    Dragging,
    /// Свободный бросок с трением.
    Decelerating,
    /// Пружина к точке прилипания.
    SnappingToPoint,
    /// Пружина возврата из-за края (после оттяжки или броска, пересёкшего край).
    ReturningFromEdge,
    /// Программная анимированная прокрутка.
    ScrollingToTarget,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSample {
    pub offset: f64,
    /// Скорость контента, offset/с. Во время перетаскивания 0: скорость пальца считает `velocity`-модуль.
    pub velocity: f64,
    pub phase: ScrollPhase,
}

/// Положение и скорость контента в один момент: то, что одна ось отдаёт другой при передаче движения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollMotionState {
    pub offset: f64,
    /// Скорость контента, offset/с.
    pub velocity: f64,
}

/// Как выполнить программную прокрутку.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTransition {
    Animated,
    Immediate,
}

/// Что запланировано при отпускании пальца.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReleaseOutcome {
    /// Скорость ниже порога, остались на месте.
    Settled { offset: f64 },
    /// Бросок остановится внутри границ в `projected_offset`.
    Coasting { projected_offset: f64 },
    /// Бросок дойдёт до края `edge_offset` и вернётся пружиной.
    CoastsPastEdge { edge_offset: f64 },
    /// Пружина к точке прилипания `index` (в отсортированном списке точек).
    SnapsToPoint { index: usize, offset: f64 },
    /// Контент был за краем: пружина вернёт его в `offset`. `snap_index` — индекс точки
    /// прилипания, в которую возвращаемся (она ближайшая к краю), или `None`, если точек нет и
    /// возврат идёт к самому краю. Индекс отдаётся готовым, чтобы потребитель не искал точку
    /// сравнением `f64 ==`.
    ReturnsFromOverscroll { offset: f64, snap_index: Option<usize> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpringPurpose {
    SnapToPoint,
    ReturnFromEdge,
    ScrollToTarget,
}

#[derive(Debug, Clone, Copy)]
enum Motion {
    Resting {
        offset: f64,
    },
    /// `origin_virtual` — «виртуальное» положение пальца в начале (контент за краем восстанавливается
    /// обратной «резинкой»), `delta` — сдвиг пальца с начала, `origin_snap` — точка, с которой начали.
    Dragging {
        origin_virtual: f64,
        delta: f64,
        origin_snap: Option<usize>,
    },
    /// Бросок; `edge_return` — заранее созданная пружина, если бросок пересечёт край.
    Decelerating {
        segment: DecelerationSegment,
        edge_return: Option<SpringAnimation>,
    },
    Springing {
        animation: SpringAnimation,
        purpose: SpringPurpose,
    },
}

/// Движок прокрутки одной оси.
#[derive(Debug, Clone)]
pub struct ScrollAxis {
    /// Конфиг как задан (без Reduce Motion): от него пересчитывается `settings` при смене режима.
    base_settings: ResolvedScrollConfig,
    /// Действующие параметры: `base_settings` с учётом `motion`.
    settings: ResolvedScrollConfig,
    motion_preference: MotionPreference,
    geometry: AxisGeometry,
    rubber: RubberBand,
    snap_points: Vec<f64>,
    motion: Motion,
}

impl ScrollAxis {
    /// Создать движок в покое на `initial_offset` (обязан лежать в границах) с полным движением.
    pub fn new(config: &ScrollConfig, geometry: AxisGeometry, initial_offset: f64) -> Result<Self, ScrollError> {
        Self::with_motion(config, MotionPreference::Full, geometry, initial_offset)
    }

    /// То же, но с предпочтением движения: при [`MotionPreference::Reduced`] пружины прилипания и
    /// возврата теряют перелёт ([`Spring::for_motion`]).
    pub fn with_motion(
        config: &ScrollConfig,
        motion: MotionPreference,
        geometry: AxisGeometry,
        initial_offset: f64,
    ) -> Result<Self, ScrollError> {
        let base_settings = config.resolve()?;
        geometry.validate()?;
        if !initial_offset.is_finite() || initial_offset < geometry.min_offset || initial_offset > geometry.max_offset {
            return Err(ScrollError::InitialOffsetOutOfBounds {
                offset: initial_offset,
                min: geometry.min_offset,
                max: geometry.max_offset,
            });
        }
        Ok(Self {
            rubber: RubberBand::new(base_settings.rubber_band_coefficient, geometry.viewport_extent)?,
            base_settings,
            settings: base_settings.for_motion(motion),
            motion_preference: motion,
            geometry,
            snap_points: Vec::new(),
            motion: Motion::Resting { offset: initial_offset },
        })
    }

    /// Текущее предпочтение движения.
    pub fn motion_preference(&self) -> MotionPreference {
        self.motion_preference
    }

    /// Сменить предпочтение движения на лету (пользователь включил Reduce Motion при открытой
    /// шкале). Действует на **новые** движения: уже запущенная пружина доезжает как была, как и в
    /// аниматоре свойств. Положение, фаза и точки прилипания не меняются.
    pub fn set_motion_preference(&mut self, motion: MotionPreference) {
        self.motion_preference = motion;
        self.settings = self.base_settings.for_motion(motion);
    }

    pub fn geometry(&self) -> AxisGeometry {
        self.geometry
    }

    /// Точки прилипания (отсортированы по возрастанию, без повторов).
    pub fn snap_points(&self) -> &[f64] {
        &self.snap_points
    }

    /// Положение, скорость и фаза в момент `now`.
    pub fn sample(&self, now: MonotonicNanos) -> ScrollSample {
        match &self.motion {
            Motion::Resting { offset } => ScrollSample {
                offset: *offset,
                velocity: 0.0,
                phase: ScrollPhase::Resting,
            },
            Motion::Dragging {
                origin_virtual, delta, ..
            } => ScrollSample {
                offset: self.displayed_offset(origin_virtual + delta),
                velocity: 0.0,
                phase: ScrollPhase::Dragging,
            },
            Motion::Decelerating { segment, edge_return } => {
                if let Some(animation) = edge_return
                    && now >= animation.start_time()
                {
                    return Self::sample_spring(animation, SpringPurpose::ReturnFromEdge, now);
                }
                let (offset, velocity, finished) = segment.sample(now);
                let phase = if finished && edge_return.is_none() {
                    ScrollPhase::Resting
                } else {
                    ScrollPhase::Decelerating
                };
                ScrollSample {
                    offset,
                    velocity,
                    phase,
                }
            }
            Motion::Springing { animation, purpose } => Self::sample_spring(animation, *purpose, now),
        }
    }

    /// Схватить контент пальцем в момент `now`: прерывает любое движение, хватает ТЕКУЩЕЕ
    /// положение. Если контент за краем (идёт возврат), виртуальное положение пальца
    /// восстанавливается обратной «резинкой» — скачка нет.
    pub fn begin_drag(&mut self, now: MonotonicNanos) {
        let offset = self.sample(now).offset;
        self.motion = Motion::Dragging {
            origin_virtual: self.virtual_position_for(offset),
            delta: 0.0,
            origin_snap: nearest_index(&self.snap_points, offset),
        };
    }

    /// Палец сдвинулся на `total_offset_delta` с начала перетаскивания. Внутри границ контент
    /// идёт 1:1, за краем — через «резинку».
    pub fn drag_to(&mut self, total_offset_delta: f64) -> Result<(), ScrollError> {
        ensure_finite("total_offset_delta", total_offset_delta)?;
        match &mut self.motion {
            Motion::Dragging { delta, .. } => {
                *delta = total_offset_delta;
                Ok(())
            }
            Motion::Resting { .. } | Motion::Decelerating { .. } | Motion::Springing { .. } => {
                Err(ScrollError::NotDragging)
            }
        }
    }

    /// Палец отпущен со скоростью `velocity` (offset/с). Выбор движения: за краем — возврат;
    /// есть точки — прилипание по проекции; иначе — свободное замедление.
    pub fn release(&mut self, velocity: f64, now: MonotonicNanos) -> Result<ReleaseOutcome, ScrollError> {
        ensure_finite("velocity", velocity)?;
        let Motion::Dragging {
            origin_virtual,
            delta,
            origin_snap,
        } = self.motion
        else {
            return Err(ScrollError::NotDragging);
        };
        let virtual_position = origin_virtual + delta;
        let offset = self.displayed_offset(virtual_position);
        // За краем контент едет медленнее пальца: скорость пальца умножается на наклон «резинки».
        let overshoot = virtual_position - self.clamp(virtual_position);
        let content_velocity = if overshoot == 0.0 {
            velocity
        } else {
            velocity * self.rubber.slope(overshoot)
        };
        self.plan_motion(offset, content_velocity, origin_snap, now)
    }

    /// Жест отменён системой: то же, что отпускание **без скорости** — тот же планировщик
    /// ([`ScrollAxis::release`] с нулём), поэтому отмена и отпускание имеют одну семантику:
    /// к точке прилипания ближайшей к месту отмены (политика `AdjacentOnly` учитывается), иначе
    /// остаёмся на месте, а за краем — возвращаемся к краю. Броска нет.
    pub fn cancel(&mut self, now: MonotonicNanos) -> Result<ReleaseOutcome, ScrollError> {
        self.release(0.0, now)
    }

    /// Программная прокрутка к `target` (обрезается границами). Прерывает любое движение,
    /// в том числе перетаскивание. `Animated` — пружиной прилипания с наследованием скорости.
    pub fn scroll_to(
        &mut self,
        target: f64,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), ScrollError> {
        ensure_finite("target", target)?;
        let target = self.clamp(target);
        match transition {
            ScrollTransition::Immediate => self.motion = Motion::Resting { offset: target },
            ScrollTransition::Animated => {
                let current = self.sample(now);
                let animation = self.start_spring(
                    self.settings.snap_spring,
                    (current.offset, target),
                    current.velocity,
                    now,
                )?;
                self.motion = Motion::Springing {
                    animation,
                    purpose: SpringPurpose::ScrollToTarget,
                };
            }
        }
        Ok(())
    }

    /// Продолжить чужое движение: контент встаёт на `offset` и пружиной едет к `target` (обрезается границами),
    /// начиная со скорости `velocity` (offset/с). Нужен, когда один движок принимает эстафету от другого (два дома
    /// одной карусели): положение и скорость совпадают с теми, что только что рисовал предыдущий владелец, поэтому
    /// скачка нет. `offset` границами НЕ обрезается: передающий мог быть за краем на «резинке».
    pub fn resume_motion_towards(
        &mut self,
        start: ScrollMotionState,
        target: f64,
        now: MonotonicNanos,
    ) -> Result<(), ScrollError> {
        ensure_finite("offset", start.offset)?;
        ensure_finite("velocity", start.velocity)?;
        ensure_finite("target", target)?;
        let target = self.clamp(target);
        let animation = self.start_spring(
            self.settings.snap_spring,
            (start.offset, target),
            start.velocity,
            now,
        )?;
        self.motion = Motion::Springing {
            animation,
            purpose: SpringPurpose::ScrollToTarget,
        };
        Ok(())
    }

    /// Сменить границы на лету. Перетаскивание продолжается (положение пальца то же, пересчитывается
    /// только отображение). Остальные движения перепланируются от текущих положения и скорости;
    /// покой за новыми границами превращается в возврат пружиной, покой внутри них остаётся
    /// покоем. При ошибке движок не меняется.
    pub fn set_geometry(&mut self, geometry: AxisGeometry, now: MonotonicNanos) -> Result<(), ScrollError> {
        let current_points = self.snap_points.clone();
        self.reconfigure(geometry, &current_points, now)
    }

    /// Задать точки прилипания (порядок любой, повторы убираются). Покой остаётся на месте до
    /// следующего отпускания; пружина прилипания в полёте перенацеливается на ближайшую новую точку.
    /// При ошибке движок не меняется.
    pub fn set_snap_points(&mut self, points: &[f64], now: MonotonicNanos) -> Result<(), ScrollError> {
        self.reconfigure(self.geometry, points, now)
    }

    /// Сменить границы **и** точки прилипания одной атомарной операцией (другая камера: меняются
    /// пределы шкалы и сетка точек вместе). Всё проверяется и перепланируется на копии; движок
    /// заменяется целиком, только если вся операция удалась, поэтому ошибка не оставляет
    /// «новые границы со старыми точками».
    pub fn reconfigure(
        &mut self,
        geometry: AxisGeometry,
        points: &[f64],
        now: MonotonicNanos,
    ) -> Result<(), ScrollError> {
        geometry.validate()?;
        if let Some(bad) = points.iter().find(|point| !point.is_finite()) {
            return Err(ScrollError::InvalidSnapPoint(*bad));
        }
        let mut next = self.clone();
        next.rubber = RubberBand::new(next.settings.rubber_band_coefficient, geometry.viewport_extent)?;
        next.geometry = geometry;
        let mut sorted = points.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted.dedup();
        next.snap_points = sorted;
        next.replan(now)?;
        *self = next;
        Ok(())
    }

    // ---- планирование ----

    /// Общий выбор движения из точки `offset` со скоростью `velocity`.
    fn plan_motion(
        &mut self,
        offset: f64,
        velocity: f64,
        origin_snap: Option<usize>,
        now: MonotonicNanos,
    ) -> Result<ReleaseOutcome, ScrollError> {
        let clamped = self.clamp(offset);
        if clamped != offset {
            // За краем: возврат к краю (или к ближайшей к нему точке прилипания).
            let snap_index = nearest_index(&self.snap_points, clamped);
            let target = snap_index
                .and_then(|index| self.snap_points.get(index).copied())
                .map_or(clamped, |point| self.clamp(point));
            let animation = self.start_spring(self.settings.edge_return_spring, (offset, target), velocity, now)?;
            self.motion = Motion::Springing {
                animation,
                purpose: SpringPurpose::ReturnFromEdge,
            };
            return Ok(ReleaseOutcome::ReturnsFromOverscroll {
                offset: target,
                snap_index,
            });
        }
        if !self.snap_points.is_empty() {
            let projection = offset + self.settings.deceleration.stopping_distance(velocity);
            if let Some(choice) =
                choose_snap_point(&self.snap_points, projection, origin_snap, self.settings.snap_selection)
            {
                let target = self.clamp(choice.offset);
                let animation = self.start_spring(self.settings.snap_spring, (offset, target), velocity, now)?;
                self.motion = Motion::Springing {
                    animation,
                    purpose: SpringPurpose::SnapToPoint,
                };
                return Ok(ReleaseOutcome::SnapsToPoint {
                    index: choice.index,
                    offset: target,
                });
            }
        }
        if velocity.abs() <= self.settings.deceleration.stop_velocity() {
            self.motion = Motion::Resting { offset };
            return Ok(ReleaseOutcome::Settled { offset });
        }
        let segment = DecelerationSegment::plan(
            self.settings.deceleration,
            (self.geometry.min_offset, self.geometry.max_offset),
            now,
            offset,
            velocity,
        );
        match segment.end() {
            DecelerationEnd::Stops => {
                self.motion = Motion::Decelerating {
                    segment,
                    edge_return: None,
                };
                Ok(ReleaseOutcome::Coasting {
                    projected_offset: segment.end_offset(),
                })
            }
            DecelerationEnd::HitsEdge { edge_offset, speed } => {
                // Пружина возврата стартует на краю с той же скоростью, что была у броска: без скачка.
                let edge_return = self.start_spring(
                    self.settings.edge_return_spring,
                    (edge_offset, edge_offset),
                    speed.copysign(velocity),
                    segment.end_time(),
                )?;
                self.motion = Motion::Decelerating {
                    segment,
                    edge_return: Some(edge_return),
                };
                Ok(ReleaseOutcome::CoastsPastEdge { edge_offset })
            }
        }
    }

    /// Перепланировать движение после смены границ или точек.
    fn replan(&mut self, now: MonotonicNanos) -> Result<(), ScrollError> {
        let current = self.sample(now);
        // Успокоившаяся пружина или доехавший бросок — это покой, а не движение: иначе
        // `retarget_spring` увёл бы стоящий контент к «ближайшей новой точке» при простой смене
        // пределов или сетки точек. В покое контент не двигается, пока лежит в новых границах.
        if current.phase == ScrollPhase::Resting && !matches!(self.motion, Motion::Dragging { .. }) {
            self.motion = Motion::Resting { offset: current.offset };
        }
        match self.motion {
            Motion::Dragging { .. } => Ok(()),
            Motion::Resting { offset } => {
                if self.clamp(offset) != offset {
                    self.plan_motion(offset, 0.0, None, now)?;
                }
                Ok(())
            }
            Motion::Decelerating {
                edge_return: Some(animation),
                ..
            } if now >= animation.start_time() => self.retarget_spring(animation, SpringPurpose::ReturnFromEdge, now),
            Motion::Decelerating { .. } => self
                .plan_motion(current.offset, current.velocity, None, now)
                .map(|_| ()),
            Motion::Springing { animation, purpose } => self.retarget_spring(animation, purpose, now),
        }
    }

    /// Новая цель пружины после смены границ/точек: обрезка границами, для прилипания — ближайшая
    /// новая точка.
    fn retarget_spring(
        &mut self,
        mut animation: SpringAnimation,
        purpose: SpringPurpose,
        now: MonotonicNanos,
    ) -> Result<(), ScrollError> {
        let old_target = animation.target();
        let snapped = match purpose {
            SpringPurpose::SnapToPoint => {
                nearest_index(&self.snap_points, old_target).and_then(|index| self.snap_points.get(index).copied())
            }
            SpringPurpose::ReturnFromEdge | SpringPurpose::ScrollToTarget => None,
        };
        let new_target = self.clamp(snapped.unwrap_or(old_target));
        if new_target != old_target {
            animation.retarget(new_target, now)?;
        }
        self.motion = Motion::Springing { animation, purpose };
        Ok(())
    }

    // ---- вспомогательное ----

    fn clamp(&self, value: f64) -> f64 {
        value.clamp(self.geometry.min_offset, self.geometry.max_offset)
    }

    /// Положение контента при «виртуальном» положении пальца: внутри границ 1:1, за краем — «резинка».
    fn displayed_offset(&self, virtual_position: f64) -> f64 {
        let clamped = self.clamp(virtual_position);
        clamped + self.rubber.stretch(virtual_position - clamped)
    }

    /// Обратное к [`Self::displayed_offset`]: где палец, если контент стоит в `offset`.
    fn virtual_position_for(&self, offset: f64) -> f64 {
        let clamped = self.clamp(offset);
        clamped + self.rubber.unstretch(offset - clamped)
    }

    fn start_spring(
        &self,
        spring: Spring,
        path: (f64, f64),
        velocity: f64,
        start_time: MonotonicNanos,
    ) -> Result<SpringAnimation, ScrollError> {
        Ok(SpringAnimation::start(SpringStart {
            spring,
            from: path.0,
            to: path.1,
            initial_velocity: velocity,
            start_time,
            rest: self.settings.rest,
        })?)
    }

    fn sample_spring(animation: &SpringAnimation, purpose: SpringPurpose, now: MonotonicNanos) -> ScrollSample {
        let sample = animation.sample(now);
        let phase = match (sample.phase, purpose) {
            (SpringPhase::AtRest, _) => ScrollPhase::Resting,
            (SpringPhase::Moving, SpringPurpose::SnapToPoint) => ScrollPhase::SnappingToPoint,
            (SpringPhase::Moving, SpringPurpose::ReturnFromEdge) => ScrollPhase::ReturningFromEdge,
            (SpringPhase::Moving, SpringPurpose::ScrollToTarget) => ScrollPhase::ScrollingToTarget,
        };
        ScrollSample {
            offset: sample.value,
            velocity: sample.velocity,
            phase,
        }
    }
}

fn ensure_finite(field: &'static str, value: f64) -> Result<(), ScrollError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ScrollError::NonFiniteInput { field, value })
    }
}
