//! Кубический корень «в форме шейдера»: без деления, `pow`, `cbrt`, `exp`, `log` — только целые операции
//! и сложение, вычитание, умножение f32. Так он одинаков на любом GPU Vulkan (OpFAdd/OpFSub/OpFMul
//! округляются точно, а `precise` запрещает слияние в FMA и перестановки).
//!
//! # Алгоритм для `x > 0`
//! 1. Начальное приближение `r ≈ x^(−1/3)` из битов числа: `bits(r) = MAGIC − bits(x) / 3` (целое деление
//!    на константу точно на любом GPU). Погрешность — до 7 %.
//! 2. Три шага Ньютона для `f(r) = r⁻³ − x`: `r ← r · (4/3 − (1/3) · x · r³)`, где `x·r³` считается СТРОГО
//!    в порядке `((x · r) · r) · r`. Ошибка падает квадратично: ~7 % → ~7·10⁻³ → ~5·10⁻⁵ → предел f32 (~4·10⁻⁷ наибольшая
//!    относительная ошибка по замеру на всём диапазоне).
//! 3. `cbrt(x) = (x · r) · r`.
//!
//! Знак переносится на результат (`cbrt(−x) = −cbrt(x)`). Если `|x| < 10⁻³⁰`, результат 0: вход не доходит
//! до денормалей (с `10⁻³⁰` значение `x·r³` остаётся порядка 1 без переполнения), а отклонение для цвета
//! — меньше 10⁻¹⁰ световой единицы, на ячейке в сотни клеток незаметно.
//!
//! GLSL:
//! ```glsl
//! precise float cube_root(float x) {
//!     float magnitude = abs(x);
//!     if (isnan(x) || magnitude < 1.0e-30) return 0.0;
//!     float r = uintBitsToFloat(0x54a21000u - floatBitsToUint(magnitude) / 3u);
//!     for (int step = 0; step < 3; ++step) {
//!         float cubed = ((magnitude * r) * r) * r;
//!         r = r * (1.3333334 - 0.33333334 * cubed);
//!     }
//!     float root = (magnitude * r) * r;
//!     return x < 0.0 ? -root : root;
//! }
//! ```
//! Вход конечный (таблица кодов и матрицы дают конечные числа); NaN и бесконечности не предусмотрены.

/// Константа начального приближения (перебором подобрана под `x^(−1/3)` на диапазоне 10⁻³⁰…4; проверена тестом).
const INITIAL_GUESS_MAGIC: u32 = 0x54a2_1000;

/// Число шагов Ньютона.
pub const NEWTON_STEPS: u32 = 3;

/// Модуль входа, ниже которого результат 0.
pub const CUBE_ROOT_ZERO_THRESHOLD: f32 = 1.0e-30;

/// Константа шага Ньютона `4/3` (округлена до f32 один раз, литералом).
const NEWTON_FOUR_THIRDS: f32 = 1.333_333_4;

/// Константа шага Ньютона `1/3` (литералом f32).
const NEWTON_ONE_THIRD: f32 = 0.333_333_34;

/// Кубический корень со знаком, детерминированный на CPU и GPU (см. описание модуля).
pub fn cube_root(value: f32) -> f32 {
    let magnitude = value.abs();
    // NaN тоже даёт 0: результат не отравляет остальные вычисления.
    if value.is_nan() || magnitude < CUBE_ROOT_ZERO_THRESHOLD {
        return 0.0;
    }
    let mut inverse_root = f32::from_bits(INITIAL_GUESS_MAGIC - magnitude.to_bits() / 3);
    for _ in 0..NEWTON_STEPS {
        let cubed = ((magnitude * inverse_root) * inverse_root) * inverse_root;
        inverse_root *= NEWTON_FOUR_THIRDS - NEWTON_ONE_THIRD * cubed;
    }
    let root = (magnitude * inverse_root) * inverse_root;
    if value < 0.0 { -root } else { root }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_f64_cube_root_across_the_working_range() {
        let mut worst: f64 = 0.0;
        // Геометрическая сетка от порога до 4: покрывает тени, середину, света и запас вне охвата.
        let mut value = 1.0e-30_f64;
        while value < 4.0 {
            let expected = value.cbrt();
            let actual = f64::from(cube_root(value as f32));
            worst = worst.max(((actual - expected) / expected).abs());
            value *= 1.37;
        }
        assert!(worst < 1.0e-6, "наибольшая относительная ошибка {worst}");
    }

    #[test]
    fn is_exact_enough_at_perfect_cubes() {
        for (value, root) in [(1.0_f32, 1.0_f32), (8.0, 2.0), (0.125, 0.5), (0.001, 0.1)] {
            let actual = cube_root(value);
            assert!(((actual - root) / root).abs() < 1.0e-6, "{value} → {actual}");
        }
    }

    #[test]
    fn zero_tiny_and_nan_give_zero() {
        assert_eq!(cube_root(0.0), 0.0);
        assert_eq!(cube_root(1.0e-31), 0.0);
        assert_eq!(cube_root(-1.0e-31), 0.0);
        assert_eq!(cube_root(f32::NAN), 0.0);
        assert!(cube_root(1.0e-30) > 0.0);
    }

    #[test]
    fn is_odd_and_monotonic() {
        let mut previous = 0.0_f32;
        let mut value = 1.0e-6_f32;
        while value < 2.0 {
            let positive = cube_root(value);
            assert_eq!(cube_root(-value), -positive);
            assert!(positive > previous, "не монотонен на {value}");
            previous = positive;
            value *= 1.01;
        }
    }
}
