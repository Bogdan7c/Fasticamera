//! Сглаживание края и слияние форм.

use crate::color::PremultipliedColor;

/// Покрытие пикселя фигурой 0..1 по расстоянию `d_px` от центра пикселя до границы (внутри < 0).
///
/// Ступенька шириной `aa_width_px` вокруг нулевой линии со сглаживанием smoothstep. В шейдере
/// `aa_width = fwidth(d)` (шаг расстояния за пиксель), на CPU — параметр. `aa_width_px ≤ 0` даёт
/// резкий край: покрыто, если `d < 0`.
pub fn coverage(d_px: f32, aa_width_px: f32) -> f32 {
    if aa_width_px <= 0.0 {
        return if d_px < 0.0 { 1.0 } else { 0.0 };
    }
    // t = 1 глубоко внутри (d ≤ −aa/2), 0 снаружи (d ≥ aa/2).
    let t = ((0.5 * aa_width_px - d_px) / aa_width_px).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Результат слияния двух полей: значение и веса, с которыми слились градиенты (в сумме 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothUnion {
    pub distance: f32,
    /// Вес первого поля в градиенте слитой формы.
    pub weight_first: f32,
    /// Вес второго поля.
    pub weight_second: f32,
}

/// Полиномиальный smooth-min: формы сливаются «как капли», когда расстояние между ними меньше `k`.
///
/// `h = max(k − |a − b|, 0) / k;  result = min(a, b) − h²·k/4`. `k` — радиус слияния, px;
/// `k ≤ 0` — обычный `min` без слияния.
pub fn smooth_union(first_px: f32, second_px: f32, merge_radius_px: f32) -> f32 {
    smooth_union_with_weights(first_px, second_px, merge_radius_px).distance
}

/// То же, что [`smooth_union`], плюс веса градиентов: градиент слитого поля — это
/// `w₁·∇d₁ + w₂·∇d₂` (аналитическая производная формулы), так считается нормаль слитой формы.
pub fn smooth_union_with_weights(first_px: f32, second_px: f32, merge_radius_px: f32) -> SmoothUnion {
    let (smaller_is_first, smaller) = if first_px <= second_px {
        (true, first_px)
    } else {
        (false, second_px)
    };
    if merge_radius_px <= 0.0 {
        return SmoothUnion {
            distance: smaller,
            weight_first: if smaller_is_first { 1.0 } else { 0.0 },
            weight_second: if smaller_is_first { 0.0 } else { 1.0 },
        };
    }
    let h = (merge_radius_px - (first_px - second_px).abs()).max(0.0) / merge_radius_px;
    // Меньшее поле сохраняет вес 1 − h/2, большее получает h/2 (∂/∂a и ∂/∂b формулы выше).
    let (weight_smaller, weight_larger) = (1.0 - 0.5 * h, 0.5 * h);
    SmoothUnion {
        distance: smaller - h * h * merge_radius_px * 0.25,
        weight_first: if smaller_is_first {
            weight_smaller
        } else {
            weight_larger
        },
        weight_second: if smaller_is_first {
            weight_larger
        } else {
            weight_smaller
        },
    }
}

/// «Поверх» для предумноженных цветов: `top + below · (1 − top.a)`. Так композиция идёт и в
/// шейдерах (blend-состояние рендера — `ONE, ONE_MINUS_SRC_ALPHA`).
pub fn over(top: PremultipliedColor, below: PremultipliedColor) -> PremultipliedColor {
    let keep = 1.0 - top.a;
    PremultipliedColor {
        r: top.r + below.r * keep,
        g: top.g + below.g * keep,
        b: top.b + below.b * keep,
        a: top.a + below.a * keep,
    }
}
