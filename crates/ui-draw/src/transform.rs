//! Аффинное преобразование плоскости: «локальные координаты фигуры → пиксели экрана».
//!
//! Матрица 2×3 (последняя строка `[0 0 1]` подразумевается), как `mat3x2` в шейдере:
//!
//! ```text
//! x' = a·x + b·y + tx
//! y' = c·x + d·y + ty
//! ```
//!
//! Оси экрана: x вправо, y вниз. Поэтому [`Affine2::rotate`] с положительным углом поворачивает
//! фигуру ПО часовой стрелке на экране.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::geometry::{PxPoint, PxVector};

/// Почему преобразование нельзя обратить или использовать.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum AffineError {
    /// В матрице есть NaN или бесконечность.
    #[error("в матрице преобразования есть нечисловое значение")]
    NotFinite,
    /// Определитель (почти) ноль: плоскость схлопнута в линию или точку, обратного нет.
    #[error("преобразование вырождено, определитель {determinant}")]
    Degenerate { determinant: f32 },
}

/// Аффинное преобразование (поворот/масштаб/сдвиг/перекос) в `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Affine2 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

/// Определитель меньше этого — считаем преобразование вырожденным. Инженерный порог: фигура
/// размером в пиксели при масштабе 1e-3 уже невидима, обращать такую матрицу в `f32` нельзя.
const DEGENERATE_DETERMINANT: f32 = 1.0e-6;

impl Affine2 {
    /// Тождественное преобразование.
    pub const IDENTITY: Affine2 = Affine2 {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// Сдвиг на `(dx, dy)` пикселей.
    pub const fn translate(dx: f32, dy: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: dx,
            ty: dy,
        }
    }

    /// Масштаб вокруг начала координат.
    pub const fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Одинаковый масштаб по обеим осям.
    pub const fn uniform_scale(factor: f32) -> Self {
        Self::scale(factor, factor)
    }

    /// Поворот вокруг начала координат на `radians`; при y вниз положительный угол — по часовой стрелке.
    pub fn rotate(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            a: cos,
            b: -sin,
            c: sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Композиция: СНАЧАЛА `self`, ПОТОМ `next` (то есть `next ∘ self`).
    ///
    /// Привычная запись «масштаб, потом поворот, потом сдвиг»:
    /// `Affine2::scale(..).then(Affine2::rotate(..)).then(Affine2::translate(..))`.
    pub fn then(self, next: Affine2) -> Affine2 {
        Affine2 {
            a: next.a * self.a + next.b * self.c,
            b: next.a * self.b + next.b * self.d,
            c: next.c * self.a + next.d * self.c,
            d: next.c * self.b + next.d * self.d,
            tx: next.a * self.tx + next.b * self.ty + next.tx,
            ty: next.c * self.tx + next.d * self.ty + next.ty,
        }
    }

    /// Определитель линейной части: со знаком площадь, на которую преобразование умножает фигуру.
    pub fn determinant(&self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    /// Все шесть чисел конечны.
    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.tx, self.ty]
            .iter()
            .all(|value| value.is_finite())
    }

    /// Проверка пригодности для рисования: числа конечны, матрицу можно обратить.
    pub fn check_invertible(&self) -> Result<(), AffineError> {
        self.inverse().map(|_| ())
    }

    /// Обратное преобразование (экран → локальные координаты).
    pub fn inverse(&self) -> Result<Affine2, AffineError> {
        if !self.is_finite() {
            return Err(AffineError::NotFinite);
        }
        let determinant = self.determinant();
        if determinant.abs() < DEGENERATE_DETERMINANT || !determinant.is_finite() {
            return Err(AffineError::Degenerate { determinant });
        }
        let inv_det = 1.0 / determinant;
        let a = self.d * inv_det;
        let b = -self.b * inv_det;
        let c = -self.c * inv_det;
        let d = self.a * inv_det;
        Ok(Affine2 {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + b * self.ty),
            ty: -(c * self.tx + d * self.ty),
        })
    }

    /// Применить к точке (с учётом сдвига).
    pub fn apply_point(&self, point: PxPoint) -> PxPoint {
        PxPoint::new(
            self.a * point.x + self.b * point.y + self.tx,
            self.c * point.x + self.d * point.y + self.ty,
        )
    }

    /// Применить к смещению (без сдвига: вектор не имеет положения).
    pub fn apply_vector(&self, vector: PxVector) -> PxVector {
        PxVector::new(
            self.a * vector.dx + self.b * vector.dy,
            self.c * vector.dx + self.d * vector.dy,
        )
    }

    /// Применить ТРАНСПОНИРОВАННУЮ линейную часть: так градиент поля переносится между системами.
    /// Если поле задано в локальных координатах, а нужен его градиент на экране, вызывать у
    /// ОБРАТНОГО преобразования (экран → локальные): `∇_экран = (L⁻¹)ᵀ · ∇_локальный`.
    pub fn apply_gradient(&self, gradient: PxVector) -> PxVector {
        PxVector::new(
            self.a * gradient.dx + self.c * gradient.dy,
            self.b * gradient.dx + self.d * gradient.dy,
        )
    }
}

impl Default for Affine2 {
    fn default() -> Self {
        Self::IDENTITY
    }
}
