//! Перевод «глаза ↔ поверхность» для положения телефона.
//!
//! Глазной прямоугольник — экран, каким его видит человек (в альбоме стороны поменяны); поверхность — портретный
//! экран рендера. Перевод — поворот вокруг общего центра на кратный 90° угол, поэтому прямоугольник переходит в
//! прямоугольник. Здесь он описан один раз: им пользуются и готовая раскладка, и её построение (безопасная зона).

use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::{Point, Rect, Size};

/// Держат ли телефон боком: стороны глазного прямоугольника поменяны местами.
pub(super) fn is_landscape(orientation: DeviceOrientation) -> bool {
    matches!(
        orientation,
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight
    )
}

/// Перевод координат для одного положения телефона и одной поверхности, pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EyeTransform {
    orientation: DeviceOrientation,
    eye_size: Size,
    surface_size: Size,
}

impl EyeTransform {
    pub(super) fn new(orientation: DeviceOrientation, surface_size: Size) -> Self {
        let eye_size = if is_landscape(orientation) {
            Size::new(surface_size.height, surface_size.width)
        } else {
            surface_size
        };
        Self {
            orientation,
            eye_size,
            surface_size,
        }
    }

    pub(super) fn orientation(&self) -> DeviceOrientation {
        self.orientation
    }

    /// Размер глазного прямоугольника, pt.
    pub(super) fn eye_size(&self) -> Size {
        self.eye_size
    }

    /// Точка глазных координат → точка поверхности.
    pub(super) fn eye_point_on_surface(&self, eye_point: Point) -> Point {
        let (dx, dy) = (
            eye_point.x - self.eye_size.width / 2.0,
            eye_point.y - self.eye_size.height / 2.0,
        );
        let (sx, sy) = match self.orientation {
            DeviceOrientation::Portrait => (dx, dy),
            // Поворот на 90° по часовой в координатах «y вниз»: (dx, dy) → (−dy, dx).
            DeviceOrientation::LandscapeLeft => (-dy, dx),
            DeviceOrientation::PortraitUpsideDown => (-dx, -dy),
            // На 90° против часовой: (dx, dy) → (dy, −dx).
            DeviceOrientation::LandscapeRight => (dy, -dx),
        };
        Point::new(self.surface_size.width / 2.0 + sx, self.surface_size.height / 2.0 + sy)
    }

    /// Точка поверхности → точка глазных координат.
    pub(super) fn surface_point_in_eye(&self, surface_point: Point) -> Point {
        let (sx, sy) = (
            surface_point.x - self.surface_size.width / 2.0,
            surface_point.y - self.surface_size.height / 2.0,
        );
        let (dx, dy) = match self.orientation {
            DeviceOrientation::Portrait => (sx, sy),
            DeviceOrientation::LandscapeLeft => (sy, -sx),
            DeviceOrientation::PortraitUpsideDown => (-sx, -sy),
            DeviceOrientation::LandscapeRight => (-sy, sx),
        };
        Point::new(self.eye_size.width / 2.0 + dx, self.eye_size.height / 2.0 + dy)
    }

    /// Глазной прямоугольник → прямоугольник поверхности.
    pub(super) fn eye_rect_on_surface(&self, eye_rect: Rect) -> Rect {
        rect_between(
            self.eye_point_on_surface(Point::new(eye_rect.min_x(), eye_rect.min_y())),
            self.eye_point_on_surface(Point::new(eye_rect.max_x(), eye_rect.max_y())),
        )
    }

    /// Прямоугольник поверхности → глазной прямоугольник. Вывернутый прямоугольник (отступы больше экрана) остаётся
    /// вывернутым: «места нет» решает построение раскладки, а не перевод координат.
    pub(super) fn surface_rect_in_eye(&self, surface_rect: Rect) -> Rect {
        let corner = self.surface_point_in_eye(Point::new(surface_rect.min_x(), surface_rect.min_y()));
        let opposite = self.surface_point_in_eye(Point::new(surface_rect.max_x(), surface_rect.max_y()));
        // Какая из двух точек левее и выше глазами, решает поворот, а не сравнение чисел: размеры сохраняют знак.
        let (left, right) = match self.orientation {
            DeviceOrientation::Portrait | DeviceOrientation::LandscapeLeft => (corner.x, opposite.x),
            DeviceOrientation::PortraitUpsideDown | DeviceOrientation::LandscapeRight => (opposite.x, corner.x),
        };
        let (top, bottom) = match self.orientation {
            DeviceOrientation::Portrait | DeviceOrientation::LandscapeRight => (corner.y, opposite.y),
            DeviceOrientation::PortraitUpsideDown | DeviceOrientation::LandscapeLeft => (opposite.y, corner.y),
        };
        Rect::from_xywh(left, top, right - left, bottom - top)
    }
}

/// Прямоугольник по двум противоположным углам в любом порядке.
fn rect_between(first: Point, second: Point) -> Rect {
    Rect::from_xywh(
        first.x.min(second.x),
        first.y.min(second.y),
        (first.x - second.x).abs(),
        (first.y - second.y).abs(),
    )
}
