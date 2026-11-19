//! Геометрия интерфейса в точках (pt).
//!
//! Все размеры, положения и смещения `ui-core` — в pt эталонного iPhone (`docs/ios-camera/layout.md`).
//! Пиксели появляются только на входе ([`crate::input::PointScale`]) и на выходе в рендер (сессия 11).
//! Тип чисел — `f64`: физика (пружины, прокрутка) считается аналитически от времени, и `f64` даёт
//! запас точности для сравнения с формулами в тестах; рендер переводит в `f32` один раз.
//!
//! Начало координат — левый верхний угол экрана, ось `y` направлена вниз (как у surface и UIKit).

use serde::{Deserialize, Serialize};

/// Точка на экране, pt.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Смещение от `origin` до этой точки.
    pub fn offset_from(self, origin: Point) -> Vector {
        Vector::new(self.x - origin.x, self.y - origin.y)
    }

    /// Точка, сдвинутая на `delta`.
    pub fn translated(self, delta: Vector) -> Point {
        Point::new(self.x + delta.dx, self.y + delta.dy)
    }

    /// Расстояние между точками, pt.
    pub fn distance_to(self, other: Point) -> f64 {
        self.offset_from(other).length()
    }

    /// Середина отрезка (фокус щипка двумя пальцами).
    pub fn midpoint(self, other: Point) -> Point {
        Point::new((self.x + other.x) * 0.5, (self.y + other.y) * 0.5)
    }
}

/// Смещение или скорость по двум осям (pt или pt/с — по контексту поля).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vector {
    pub dx: f64,
    pub dy: f64,
}

impl Vector {
    pub const ZERO: Vector = Vector { dx: 0.0, dy: 0.0 };

    pub const fn new(dx: f64, dy: f64) -> Self {
        Self { dx, dy }
    }

    /// Длина вектора.
    pub fn length(self) -> f64 {
        self.dx.hypot(self.dy)
    }

    /// Вектор, умноженный на число.
    pub fn scaled(self, factor: f64) -> Vector {
        Vector::new(self.dx * factor, self.dy * factor)
    }
}

/// Размер, pt. Отрицательный размер не имеет смысла: конструкторы вёрстки обязаны его не создавать.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const ZERO: Size = Size {
        width: 0.0,
        height: 0.0,
    };

    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// Прямоугольник без поворота: левый верхний угол + размер, pt.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    /// Прямоугольник по левому верхнему углу и размерам.
    pub const fn from_xywh(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(width, height),
        }
    }

    /// Прямоугольник заданного размера с центром в `center`.
    pub fn centered_at(center: Point, size: Size) -> Self {
        Self::from_xywh(
            center.x - size.width * 0.5,
            center.y - size.height * 0.5,
            size.width,
            size.height,
        )
    }

    pub fn min_x(&self) -> f64 {
        self.origin.x
    }

    pub fn min_y(&self) -> f64 {
        self.origin.y
    }

    pub fn max_x(&self) -> f64 {
        self.origin.x + self.size.width
    }

    pub fn max_y(&self) -> f64 {
        self.origin.y + self.size.height
    }

    pub fn center(&self) -> Point {
        Point::new(
            self.origin.x + self.size.width * 0.5,
            self.origin.y + self.size.height * 0.5,
        )
    }

    /// Содержит ли прямоугольник точку. Левая и верхняя границы включены, правая и нижняя — нет:
    /// две соседние рамки без зазора не делят одну точку касания.
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.min_x() && point.x < self.max_x() && point.y >= self.min_y() && point.y < self.max_y()
    }

    /// Прямоугольник, расширенный наружу на `outsets` (отрицательные значения сужают).
    pub fn outset_by(&self, outsets: Insets) -> Rect {
        Rect::from_xywh(
            self.origin.x - outsets.left,
            self.origin.y - outsets.top,
            self.size.width + outsets.left + outsets.right,
            self.size.height + outsets.top + outsets.bottom,
        )
    }

    /// Прямоугольник, сжатый внутрь на `insets` (безопасная зона, отступы).
    pub fn inset_by(&self, insets: Insets) -> Rect {
        self.outset_by(Insets::new(-insets.top, -insets.left, -insets.bottom, -insets.right))
    }
}

/// Отступы по четырём сторонам, pt (безопасная зона, расширение области касания).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Insets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

impl Insets {
    pub const ZERO: Insets = Insets {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    };

    pub const fn new(top: f64, left: f64, bottom: f64, right: f64) -> Self {
        Self {
            top,
            left,
            bottom,
            right,
        }
    }

    /// Одинаковый отступ со всех сторон.
    pub const fn uniform(value: f64) -> Self {
        Self::new(value, value, value, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_is_half_open() {
        let rect = Rect::from_xywh(10.0, 20.0, 30.0, 40.0);
        assert!(rect.contains(Point::new(10.0, 20.0)));
        assert!(rect.contains(Point::new(39.999, 59.999)));
        assert!(!rect.contains(Point::new(40.0, 30.0)));
        assert!(!rect.contains(Point::new(20.0, 60.0)));
        assert!(!rect.contains(Point::new(9.999, 30.0)));
    }

    #[test]
    fn outset_and_inset_are_inverse() {
        let rect = Rect::from_xywh(10.0, 20.0, 30.0, 40.0);
        let insets = Insets::new(1.0, 2.0, 3.0, 4.0);
        let grown = rect.outset_by(insets);
        assert_eq!(grown, Rect::from_xywh(8.0, 19.0, 36.0, 44.0));
        assert_eq!(grown.inset_by(insets), rect);
    }

    #[test]
    fn centered_rect_has_requested_center() {
        let rect = Rect::centered_at(Point::new(100.0, 50.0), Size::new(20.0, 10.0));
        assert_eq!(rect, Rect::from_xywh(90.0, 45.0, 20.0, 10.0));
        assert_eq!(rect.center(), Point::new(100.0, 50.0));
    }

    #[test]
    fn distance_and_midpoint() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(3.0, 4.0);
        assert_eq!(a.distance_to(b), 5.0);
        assert_eq!(a.midpoint(b), Point::new(1.5, 2.0));
        assert_eq!(b.offset_from(a), Vector::new(3.0, 4.0));
    }
}
