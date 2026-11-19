//! Общий привод оси: связь «жесты экрана → движок прокрутки `ui-core`» для шкалы и карусели.
//!
//! Один движок ([`ui_core::scroll::ScrollAxis`]) и одно место, где решается всё, что относится к
//! ощущению «ведения за пальцем» (правило каталога «без дублирования»):
//! * **знак и ось** — в какую сторону должен идти палец, чтобы смещение росло ([`GrowthDirection`]);
//! * **начало отсчёта** смещения пальца ([`DragOrigin`]) — без скачка на гистерезис;
//! * **касание ловит движение**: палец на едущем контенте останавливает его там, где он есть,
//!   без скачка (контент переходит в «удерживаемое» состояние, а не в покой в целевой точке);
//! * **сборка из окружения**: токены → конфиг прокрутки роли → движок с предпочтением движения
//!   ([`AxisDrive::from_environment`]) — одна для шкалы и карусели;
//! * **одно правило отмены жеста**: системная отмена — это отпускание без скорости через общий
//!   планировщик движка; результат ([`ReleaseOutcome`]) элемент применяет так же, как после
//!   обычного отпускания (шкала — магнит, карусель — выбор ближайшего элемента);
//! * **атомарная смена границ и точек** ([`AxisDrive::reconfigure`]).
//!
//! Элементы НЕ владеют ареной жестов: экран подписывает распознаватели и передаёт сюда данные
//! жеста намерениями (`catch`, `drag_began`, `drag_moved`, `drag_ended`, `drag_cancelled`).
//! Поэтому элемент не зависит от типа намерения `I` экрана.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
pub use ui_core::scroll::ScrollTransition;
use ui_core::scroll::{AxisGeometry, ReleaseOutcome, ScrollAxis, ScrollError, ScrollPhase, ScrollSample};
use ui_core::time::{MonotonicNanos, MotionPreference};

use crate::environment::ElementEnvironment;
use crate::tokens::{SnapRole, TokenError};

/// Ошибка сборки или перенастройки привода: токены или движок прокрутки. Элементы переводят её в
/// свои ошибки (`From`), причина сохраняется.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DriveError {
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
    #[error("прокрутка: {0}")]
    Scroll(#[from] ScrollError),
}

/// Что элемент просит у привода при создании: роль (какие пружины и политика прилипания) и правила
/// сопоставления жестов.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DriveSetup {
    pub role: SnapRole,
    pub direction: GrowthDirection,
    pub drag_origin: DragOrigin,
}

/// Допустимый отрезок смещения контента, pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OffsetRange {
    pub min: f64,
    pub max: f64,
}

/// Ось экрана, вдоль которой едет контент.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAxis {
    Horizontal,
    Vertical,
}

/// В какую сторону должен идти палец, чтобы смещение (значение шкалы, номер элемента) росло.
/// Одновременно задаёт ось и знак. Пример: колесо, у которого большие значения справа, —
/// `FingerLeft` (контент едет за пальцем влево, справа подъезжают большие значения).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrowthDirection {
    FingerLeft,
    FingerRight,
    FingerUp,
    FingerDown,
}

impl GrowthDirection {
    /// Ось, вдоль которой едет контент.
    pub const fn axis(self) -> ScreenAxis {
        match self {
            GrowthDirection::FingerLeft | GrowthDirection::FingerRight => ScreenAxis::Horizontal,
            GrowthDirection::FingerUp | GrowthDirection::FingerDown => ScreenAxis::Vertical,
        }
    }

    /// Смещение контента из смещения/скорости пальца по экранным осям (`x` вправо, `y` вниз).
    pub fn content_component(self, finger: Vector) -> f64 {
        match self {
            GrowthDirection::FingerLeft => -finger.dx,
            GrowthDirection::FingerRight => finger.dx,
            GrowthDirection::FingerUp => -finger.dy,
            GrowthDirection::FingerDown => finger.dy,
        }
    }
}

/// От какой точки считать смещение пальца при перетаскивании.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragOrigin {
    /// От точки, где жест распознан (`from_recognition`): в момент `Began` смещение 0, контент
    /// не «прыгает» на путь гистерезиса и дальше идёт за пальцем 1:1. Выбор по умолчанию.
    #[default]
    FromRecognition,
    /// От точки касания (`from_touch_down`, как `translation(in:)` в UIKit): на `Began` контент
    /// сразу догоняет палец, то есть делает скачок на пройденный гистерезис.
    FromTouchDown,
}

/// Что делает контент элемента (без деталей движка).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionPhase {
    /// Стоит.
    Resting,
    /// Под пальцем (идёт за ним или удерживается неподвижно).
    Touched,
    /// Движется сам: бросок, прилипание, возврат от края, программная прокрутка.
    Settling,
}

impl MotionPhase {
    pub(crate) fn from_scroll(phase: ScrollPhase) -> Self {
        match phase {
            ScrollPhase::Resting => MotionPhase::Resting,
            ScrollPhase::Dragging => MotionPhase::Touched,
            ScrollPhase::Decelerating
            | ScrollPhase::SnappingToPoint
            | ScrollPhase::ReturningFromEdge
            | ScrollPhase::ScrollingToTarget => MotionPhase::Settling,
        }
    }
}

/// Привод оси: `ScrollAxis` + правила сопоставления жестов.
#[derive(Debug, Clone)]
pub(crate) struct AxisDrive {
    scroll: ScrollAxis,
    direction: GrowthDirection,
    drag_origin: DragOrigin,
    /// Размер видимой области вдоль оси, pt (из окружения при создании): от него зависит «резинка».
    viewport_extent_points: f64,
}

impl AxisDrive {
    /// Собрать привод из окружения: проверить токены, взять конфиг прокрутки роли, создать
    /// движок с предпочтением движения. Единственное место этой цепочки для всех элементов.
    pub(crate) fn from_environment(
        environment: &ElementEnvironment<'_>,
        setup: DriveSetup,
        range: OffsetRange,
        initial_offset: f64,
    ) -> Result<Self, DriveError> {
        environment.tokens.validate()?;
        let config = environment.tokens.motion.scroll_config(setup.role)?;
        let viewport_extent_points = environment.viewport_extent_points;
        let geometry = Self::geometry_of(range, viewport_extent_points);
        Ok(Self {
            scroll: ScrollAxis::with_motion(&config, environment.motion, geometry, initial_offset)?,
            direction: setup.direction,
            drag_origin: setup.drag_origin,
            viewport_extent_points,
        })
    }

    fn geometry_of(range: OffsetRange, viewport_extent_points: f64) -> AxisGeometry {
        AxisGeometry {
            min_offset: range.min,
            max_offset: range.max,
            viewport_extent: viewport_extent_points,
        }
    }

    /// Сменить предпочтение движения на лету (Reduce Motion включили при открытом элементе).
    /// Действует на новые движения; положение и фаза не меняются.
    pub(crate) fn set_motion(&mut self, motion: MotionPreference) {
        self.scroll.set_motion_preference(motion);
    }

    pub(crate) fn motion(&self) -> MotionPreference {
        self.scroll.motion_preference()
    }

    pub(crate) fn sample(&self, now: MonotonicNanos) -> ScrollSample {
        self.scroll.sample(now)
    }

    pub(crate) fn phase(&self, now: MonotonicNanos) -> MotionPhase {
        MotionPhase::from_scroll(self.sample(now).phase)
    }

    /// Палец коснулся: контент останавливается на текущем положении. Работает и за краем
    /// (обратная «резинка» восстанавливает положение пальца): скачка нет.
    pub(crate) fn catch(&mut self, now: MonotonicNanos) {
        self.scroll.begin_drag(now);
    }

    /// Жест перетаскивания распознан.
    pub(crate) fn drag_began(&mut self, now: MonotonicNanos, translation: PanTranslation) -> Result<(), ScrollError> {
        self.scroll.begin_drag(now);
        self.drag_moved(translation)
    }

    /// Палец сдвинулся.
    pub(crate) fn drag_moved(&mut self, translation: PanTranslation) -> Result<(), ScrollError> {
        let finger = match self.drag_origin {
            DragOrigin::FromRecognition => translation.from_recognition,
            DragOrigin::FromTouchDown => translation.from_touch_down,
        };
        self.scroll.drag_to(self.direction.content_component(finger))
    }

    /// Палец отпущен с экранной скоростью `finger_velocity` (pt/с).
    pub(crate) fn drag_ended(
        &mut self,
        finger_velocity: Vector,
        now: MonotonicNanos,
    ) -> Result<ReleaseOutcome, ScrollError> {
        self.scroll
            .release(self.direction.content_component(finger_velocity), now)
    }

    /// Жест отменён системой: **то же, что отпускание без скорости** (общий планировщик движка).
    /// Элемент применяет возвращённый итог так же, как итог обычного отпускания.
    pub(crate) fn drag_cancelled(&mut self, now: MonotonicNanos) -> Result<ReleaseOutcome, ScrollError> {
        self.scroll.cancel(now)
    }

    /// Палец, поймавший контент, отпущен без перетаскивания: доехать до упора без скорости.
    pub(crate) fn release_held(&mut self, now: MonotonicNanos) -> Result<ReleaseOutcome, ScrollError> {
        self.scroll.release(0.0, now)
    }

    pub(crate) fn scroll_to(
        &mut self,
        target: f64,
        transition: ScrollTransition,
        now: MonotonicNanos,
    ) -> Result<(), ScrollError> {
        self.scroll.scroll_to(target, transition, now)
    }

    /// Сменить отрезок смещений и точки прилипания одной атомарной операцией: при ошибке привод
    /// не меняется.
    pub(crate) fn reconfigure(
        &mut self,
        range: OffsetRange,
        snap_points: &[f64],
        now: MonotonicNanos,
    ) -> Result<(), ScrollError> {
        let geometry = Self::geometry_of(range, self.viewport_extent_points);
        self.scroll.reconfigure(geometry, snap_points, now)
    }

    /// Задать только точки прилипания (при создании элемента).
    pub(crate) fn set_snap_points(&mut self, points: &[f64], now: MonotonicNanos) -> Result<(), ScrollError> {
        self.scroll.set_snap_points(points, now)
    }
}
