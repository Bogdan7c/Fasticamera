//! Геометрия в пикселях экрана (`f32`).
//!
//! Внутри `ui-core` всё в pt и `f64` (точность сверки с формулами пружин); рисование работает в
//! пикселях и `f32`, как шейдер. Перевод pt → px делается один раз, по `ui_core::input::PointScale`.
//! Начало координат — левый верхний угол логического экрана (портрет), ось y — вниз.

use serde::{Deserialize, Serialize};

/// Точка в пикселях экрана.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PxPoint {
    pub x: f32,
    pub y: f32,
}

/// Смещение в пикселях.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PxVector {
    pub dx: f32,
    pub dy: f32,
}

/// Размер в пикселях (неотрицательный — проверяет тот, кто строит фигуру).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PxSize {
    pub width: f32,
    pub height: f32,
}

/// Прямоугольник со сторонами вдоль осей: левый верхний угол + размер.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PxRect {
    pub origin: PxPoint,
    pub size: PxSize,
}

impl PxPoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl PxSize {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

impl PxRect {
    pub const fn new(origin: PxPoint, size: PxSize) -> Self {
        Self { origin, size }
    }

    /// Правая граница.
    pub fn max_x(&self) -> f32 {
        self.origin.x + self.size.width
    }

    /// Нижняя граница.
    pub fn max_y(&self) -> f32 {
        self.origin.y + self.size.height
    }

    /// Центр прямоугольника.
    pub fn center(&self) -> PxPoint {
        PxPoint::new(
            self.origin.x + self.size.width * 0.5,
            self.origin.y + self.size.height * 0.5,
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Арифметика. Добавлена сессией 11 (пакет B); существующие поля и конструкторы выше не менялись.
// ---------------------------------------------------------------------------------------------

impl PxVector {
    /// Нулевое смещение.
    pub const ZERO: PxVector = PxVector { dx: 0.0, dy: 0.0 };

    pub const fn new(dx: f32, dy: f32) -> Self {
        Self { dx, dy }
    }

    /// Длина вектора.
    pub fn length(self) -> f32 {
        self.dx.hypot(self.dy)
    }

    /// Скалярное произведение.
    pub fn dot(self, other: PxVector) -> f32 {
        self.dx * other.dx + self.dy * other.dy
    }

    /// Вектор того же направления единичной длины; `None` для нулевого (направления нет).
    pub fn normalized(self) -> Option<PxVector> {
        let length = self.length();
        if length > f32::EPSILON && length.is_finite() {
            Some(PxVector::new(self.dx / length, self.dy / length))
        } else {
            None
        }
    }

    /// Все компоненты конечны (не NaN и не бесконечность).
    pub fn is_finite(self) -> bool {
        self.dx.is_finite() && self.dy.is_finite()
    }
}

impl std::ops::Add for PxVector {
    type Output = PxVector;
    fn add(self, rhs: PxVector) -> PxVector {
        PxVector::new(self.dx + rhs.dx, self.dy + rhs.dy)
    }
}

impl std::ops::Sub for PxVector {
    type Output = PxVector;
    fn sub(self, rhs: PxVector) -> PxVector {
        PxVector::new(self.dx - rhs.dx, self.dy - rhs.dy)
    }
}

impl std::ops::Neg for PxVector {
    type Output = PxVector;
    fn neg(self) -> PxVector {
        PxVector::new(-self.dx, -self.dy)
    }
}

impl std::ops::Mul<f32> for PxVector {
    type Output = PxVector;
    fn mul(self, rhs: f32) -> PxVector {
        PxVector::new(self.dx * rhs, self.dy * rhs)
    }
}

impl PxPoint {
    /// Начало координат.
    pub const ORIGIN: PxPoint = PxPoint { x: 0.0, y: 0.0 };

    /// Все компоненты конечны.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Точка минус точка — смещение между ними.
impl std::ops::Sub for PxPoint {
    type Output = PxVector;
    fn sub(self, rhs: PxPoint) -> PxVector {
        PxVector::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// Точка плюс смещение — сдвинутая точка.
impl std::ops::Add<PxVector> for PxPoint {
    type Output = PxPoint;
    fn add(self, rhs: PxVector) -> PxPoint {
        PxPoint::new(self.x + rhs.dx, self.y + rhs.dy)
    }
}

impl PxSize {
    /// Меньшая сторона: от неё считаются «доли высоты» формы и предел радиуса скругления.
    pub fn min_side(self) -> f32 {
        self.width.min(self.height)
    }

    /// Обе стороны конечны и неотрицательны.
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width >= 0.0 && self.height >= 0.0
    }
}

impl PxRect {
    /// Прямоугольник заданного размера с центром в точке.
    pub fn from_center(center: PxPoint, size: PxSize) -> Self {
        Self {
            origin: PxPoint::new(center.x - size.width * 0.5, center.y - size.height * 0.5),
            size,
        }
    }

    /// Прямоугольник, раздутый на `margin` со всех сторон (отрицательный — сжатый до нуля).
    pub fn expanded(&self, margin: f32) -> PxRect {
        let width = (self.size.width + 2.0 * margin).max(0.0);
        let height = (self.size.height + 2.0 * margin).max(0.0);
        PxRect::from_center(self.center(), PxSize::new(width, height))
    }

    /// Наименьший прямоугольник, содержащий оба.
    pub fn union(&self, other: &PxRect) -> PxRect {
        let min_x = self.origin.x.min(other.origin.x);
        let min_y = self.origin.y.min(other.origin.y);
        let max_x = self.max_x().max(other.max_x());
        let max_y = self.max_y().max(other.max_y());
        PxRect::new(PxPoint::new(min_x, min_y), PxSize::new(max_x - min_x, max_y - min_y))
    }

    /// Пересечение двух прямоугольников; `None`, если общей площади нет (прямоугольники только касаются
    /// или не пересекаются, либо один из них пуст).
    pub fn intersection(&self, other: &PxRect) -> Option<PxRect> {
        let min_x = self.origin.x.max(other.origin.x);
        let min_y = self.origin.y.max(other.origin.y);
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        // Сравнение «не больше» ложно для NaN, поэтому нечисловой результат тоже даёт `None`.
        if max_x > min_x && max_y > min_y {
            Some(PxRect::new(
                PxPoint::new(min_x, min_y),
                PxSize::new(max_x - min_x, max_y - min_y),
            ))
        } else {
            None
        }
    }

    /// Наименьший прямоугольник, содержащий все точки; `None` для пустого набора.
    pub fn bounding(points: impl IntoIterator<Item = PxPoint>) -> Option<PxRect> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (first.x, first.y, first.x, first.y);
        for point in iter {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
        Some(PxRect::new(
            PxPoint::new(min_x, min_y),
            PxSize::new(max_x - min_x, max_y - min_y),
        ))
    }

    /// Точка лежит внутри (границы включены).
    pub fn contains(&self, point: PxPoint) -> bool {
        point.x >= self.origin.x && point.x <= self.max_x() && point.y >= self.origin.y && point.y <= self.max_y()
    }
}
