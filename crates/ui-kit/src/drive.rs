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
//! * **отмена жеста**: [`AxisDrive::drag_cancelled`] — отпускание без скорости через общий
//!   планировщик движка (так делает шкала: магнит). Карусель отмену так не трактует: «без
//!   действия» для неё — вернуться к выбранному до жеста (`SnapCarousel::drag_cancelled`);
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

/// Зона «держания» под пальцем: контент встаёт на `centre_offset` и стоит, пока палец проходит путь шириной
/// `2 × half_width`; дальше контент снова идёт за пальцем, но отставая на эту ширину. Скачков нет ни на входе,
/// ни на выходе: положение непрерывно.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HoldZone {
    /// Смещение контента, на котором зона держит (положение нуля шкалы), pt.
    pub centre_offset: f64,
    /// Половина ширины зоны, pt пути пальца.
    pub half_width: f64,
}

impl HoldZone {
    /// Смещение контента при желаемом (по пальцу) смещении `finger_offset`, если палец взял контент на
    /// `start_offset`.
    ///
    /// Зона «съедает» путь только когда значение ПРИХОДИТ к центру (палец пересекает центр или возвращается к
    /// нему с другой стороны), поэтому всё считается от стороны старта:
    /// * старт по одну сторону от центра: до центра контент идёт 1:1 (на старте нет скачка), у центра встаёт на
    ///   `2 × half_width` пути, дальше идёт за пальцем с отставанием на эту ширину; движение назад к стороне
    ///   старта отставания не имеет (оно возвращается);
    /// * старт ровно на центре (нейтраль): сторона неизвестна, зоны нет — контент идёт за пальцем 1:1, иначе у
    ///   ручки был бы «мёртвый» ход на старте от нуля (решение 247). Сторону «отхода» привод запоминает по
    ///   первому сдвигу ([`AxisDrive`]) и подставляет сюда как `start_offset`.
    pub(crate) fn applied_to(self, start_offset: f64, finger_offset: f64) -> f64 {
        let start_side = start_offset - self.centre_offset;
        if start_side == 0.0 {
            return finger_offset;
        }
        // Зеркалим так, чтобы старт был «наверху» (по знаку стороны): тогда одна формула на обе стороны.
        let sign = start_side.signum();
        let from_centre = (finger_offset - self.centre_offset) * sign;
        let held_width = 2.0 * self.half_width;
        let mapped = if from_centre >= 0.0 {
            from_centre
        } else if from_centre >= -held_width {
            0.0
        } else {
            from_centre + held_width
        };
        self.centre_offset + mapped * sign
    }
}

/// Привод оси: `ScrollAxis` + правила сопоставления жестов.
#[derive(Debug, Clone)]
pub(crate) struct AxisDrive {
    scroll: ScrollAxis,
    direction: GrowthDirection,
    drag_origin: DragOrigin,
    /// Зона держания под пальцем (магнит нуля); `None` — палец ведёт контент 1:1.
    hold_zone: Option<HoldZone>,
    /// Смещение контента в момент, когда палец взял контент: от него считается путь пальца.
    drag_start_offset: f64,
    /// Смещение, от стороны которого зона считает «откуда пришёл палец». Обычно равно `drag_start_offset`; если
    /// контент взят ровно на центре зоны, сторона определяется первым сдвигом пальца (сдвиг от нуля не держится,
    /// возврат к нулю с той же стороны — держится).
    hold_side_reference: f64,
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
            hold_zone: None,
            drag_start_offset: initial_offset,
            hold_side_reference: initial_offset,
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
        self.remember_drag_start(now);
        self.scroll.begin_drag(now);
    }

    /// Жест перетаскивания распознан.
    pub(crate) fn drag_began(&mut self, now: MonotonicNanos, translation: PanTranslation) -> Result<(), ScrollError> {
        self.remember_drag_start(now);
        self.scroll.begin_drag(now);
        self.drag_moved(translation)
    }

    /// Палец сдвинулся.
    pub(crate) fn drag_moved(&mut self, translation: PanTranslation) -> Result<(), ScrollError> {
        let finger = match self.drag_origin {
            DragOrigin::FromRecognition => translation.from_recognition,
            DragOrigin::FromTouchDown => translation.from_touch_down,
        };
        let finger_delta = self.direction.content_component(finger);
        let content_delta = match self.hold_zone {
            None => finger_delta,
            Some(zone) => {
                self.learn_departure_side(zone, finger_delta);
                // Желаемое положение по пальцу проходит через зону, обратно — смещение от точки захвата.
                zone.applied_to(self.hold_side_reference, self.drag_start_offset + finger_delta)
                    - self.drag_start_offset
            }
        };
        self.scroll.drag_to(content_delta)
    }

    /// Запомнить положение контента в момент захвата; сторона зоны пока равна ему.
    fn remember_drag_start(&mut self, now: MonotonicNanos) {
        self.drag_start_offset = self.scroll.sample(now).offset;
        self.hold_side_reference = self.drag_start_offset;
    }

    /// Контент взят ровно на центре зоны: сторону «отхода» задаёт первый ненулевой сдвиг пальца. Пока сторона не
    /// выбрана, зона не действует (`applied_to` отдаёт путь пальца как есть) — нет мёртвого хода на старте.
    fn learn_departure_side(&mut self, zone: HoldZone, finger_delta: f64) {
        let started_on_centre = self.hold_side_reference == zone.centre_offset;
        if started_on_centre && finger_delta != 0.0 {
            self.hold_side_reference = zone.centre_offset + finger_delta.signum();
        }
    }

    /// Включить зону держания под пальцем (магнит нуля); `None` — выключить. Задаётся при сборке элемента.
    pub(crate) fn set_hold_zone(&mut self, zone: Option<HoldZone>) {
        self.hold_zone = zone;
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

    /// Отпускание без скорости через общий планировщик движка. Используется там, где отмена
    /// жеста приравнена к отпусканию (шкала); карусель вместо этого возвращается к прежнему выбору.
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
