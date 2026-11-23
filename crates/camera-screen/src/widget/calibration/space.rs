//! Система координат раздела калибровки: «глазное» пространство, корневой узел и перевод жестов.
//!
//! Раздел раскладывается в глазных координатах (`CalibrationLayout`): в портрете они совпадают с поверхностью, в
//! альбоме ширина и высота меняются местами. Все узлы раздела лежат под ОДНИМ корнем, рамка которого — глазной
//! прямоугольник, поставленный центром в центр поверхности, а сам корень повёрнут вокруг центра на угол
//! раскладки (решение 202). Дети хранят рамки в системе корня («глазные координаты + сдвиг корня»), поэтому
//! хит-тест `ui-core` и список рисования поворачивают всё сами.
//!
//! Жесты приходят в координатах поверхности. Для всего, что считает смещение пальца (линейки, список,
//! свайпы), смещение переводится в глазные оси здесь — один раз и в одном месте:
//! * [`SectionSpace::eye_pan`] поворачивает смещения и скорость пана;
//! * [`SectionSpace::pan_direction`] выбирает направление распознавателя на поверхности для глазной оси.

use ui_core::geometry::{Point, Rect, Vector};
use ui_core::gesture::{PanDirection, PanTranslation};
use ui_kit::pan::PanUpdate;

use crate::calibration::CalibrationLayout;

/// Глазная ось, вдоль которой элемент ждёт пан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EyeAxis {
    /// Слева направо глазами (линейки, карусели, свайп прибора).
    Horizontal,
    /// Сверху вниз глазами (прокрутка списка).
    Vertical,
}

/// Глазное пространство раздела на конкретной поверхности.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SectionSpace {
    layout: CalibrationLayout,
    /// Левый верхний угол корня в системе поверхности до поворота: глазной прямоугольник центром в центре экрана.
    root_origin: Point,
}

impl SectionSpace {
    /// Пространство раздела; `surface` — рамка поверхности в pt (начало в нуле).
    pub(crate) fn new(layout: &CalibrationLayout, surface: Rect) -> Self {
        let eye = layout.eye_size();
        let center = surface.center();
        Self {
            layout: *layout,
            root_origin: Point::new(center.x - eye.width / 2.0, center.y - eye.height / 2.0),
        }
    }

    /// Рамка корневого узла (до поворота).
    pub(crate) fn root_frame(&self) -> Rect {
        let eye = self.layout.eye_size();
        Rect::from_xywh(self.root_origin.x, self.root_origin.y, eye.width, eye.height)
    }

    /// Угол поворота корня, радианы по часовой (как `Affine2::rotate` и свойство узла `Rotation`).
    pub(crate) fn rotation_radians(&self) -> f64 {
        f64::from(self.layout.rotation_clockwise_degrees()).to_radians()
    }

    /// Глазной прямоугольник → рамка в системе корня (там, где живут рамки узлов раздела).
    pub(crate) fn scene_rect(&self, eye_rect: Rect) -> Rect {
        Rect::from_xywh(
            eye_rect.min_x() + self.root_origin.x,
            eye_rect.min_y() + self.root_origin.y,
            eye_rect.size.width,
            eye_rect.size.height,
        )
    }

    /// Глазная точка → точка в системе корня.
    pub(crate) fn scene_point(&self, eye_point: Point) -> Point {
        Point::new(eye_point.x + self.root_origin.x, eye_point.y + self.root_origin.y)
    }

    /// Смещение на поверхности → глазное смещение. Поворот линейный, поэтому достаточно образа вектора.
    pub(crate) fn eye_vector(&self, surface_vector: Vector) -> Vector {
        let origin = self.layout.surface_point_in_eye(Point::ORIGIN);
        let moved = self
            .layout
            .surface_point_in_eye(Point::new(surface_vector.dx, surface_vector.dy));
        Vector::new(moved.x - origin.x, moved.y - origin.y)
    }

    /// Обновление пана в осях поверхности → то же в глазных осях: смещения и скорость отпускания.
    pub(crate) fn eye_pan(&self, update: PanUpdate) -> PanUpdate {
        match update {
            PanUpdate::Began { translation } => PanUpdate::Began {
                translation: self.eye_translation(translation),
            },
            PanUpdate::Changed { translation } => PanUpdate::Changed {
                translation: self.eye_translation(translation),
            },
            PanUpdate::Ended { velocity } => PanUpdate::Ended {
                velocity: self.eye_vector(velocity),
            },
            PanUpdate::Cancelled => PanUpdate::Cancelled,
        }
    }

    /// Оба отсчёта смещения в глазных осях.
    pub(crate) fn eye_translation(&self, translation: PanTranslation) -> PanTranslation {
        PanTranslation {
            from_touch_down: self.eye_vector(translation.from_touch_down),
            from_recognition: self.eye_vector(translation.from_recognition),
        }
    }

    /// Направление распознавателя ПАНА на поверхности для глазной оси: в альбоме глазная горизонталь — это
    /// вертикаль поверхности и наоборот.
    pub(crate) fn pan_direction(&self, axis: EyeAxis) -> PanDirection {
        let rotated_quarter_turn = (self.rotation_radians().sin().abs() - 1.0).abs() < 1e-6;
        match (axis, rotated_quarter_turn) {
            (EyeAxis::Horizontal, false) | (EyeAxis::Vertical, true) => PanDirection::Horizontal,
            (EyeAxis::Horizontal, true) | (EyeAxis::Vertical, false) => PanDirection::Vertical,
        }
    }
}
