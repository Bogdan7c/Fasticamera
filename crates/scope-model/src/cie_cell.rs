//! Точное деление в ячейку диаграммы CIE: `ячейка = floor(t / S)` без деления с неизвестным округлением.
//!
//! # Зачем
//! Цветность — частное (`X/(X+Y+Z)`). Деление на GPU НЕ округляется точно (и у разных драйверов по-разному), поэтому
//! счётчики CPU и GPU разошлись бы на ячейках у границы. Правило даёт ТОТ ЖЕ ответ на любом устройстве, потому что
//! определяет ячейку только умножениями (`OpFMul` округляется точно, `precise` запрещает слияние в FMA):
//!
//! ячейка `c` — НАИБОЛЬШЕЕ целое `c ≥ 0` с `fl(c · S) ≤ t` (`t` — числитель, уже сдвинутый и умноженный на клетки на единицу, `S` — знаменатель).
//!
//! Произведения `c · S` монотонны по `c`, поэтому такое `c` единственно. Приближение `floor(t · (1/S))` ошибается не
//! больше чем на одну клетку (относительная ошибка деления — единицы ULP при `c ≤ 1024`), и ОДНА поправка в каждую
//! сторону находит точный ответ при любом таком приближении ([`settle_cell`]). Сам запасной ход — не вычисление, а
//! проверка двух произведений.
//!
//! GLSL (`scope_accumulate_cie_rule.glsl`) повторяет порядок: `1/S`, `t·(1/S)`, `floor`, `min(…, size+1)`, поправка.

/// Окончательная ячейка по приближению `guess` (целое число в f32, `0 ≤ guess ≤ size + 1`): одна поправка вниз или вверх.
/// Вход: `numerator` (`t ≥ 0`) и `denominator` (`S > 0`) конечные. Результат — наибольшее `c`, где `fl(c·S) ≤ t`,
/// если `guess` отличается от него не больше чем на 1.
pub(crate) fn settle_cell(guess: f32, numerator: f32, denominator: f32) -> f32 {
    let guess_product = guess * denominator;
    if guess_product > numerator {
        return guess - 1.0;
    }
    let next_product = (guess + 1.0) * denominator;
    if next_product <= numerator { guess + 1.0 } else { guess }
}

/// Начальное приближение ячейки: `floor(t · (1/S))`, не больше `size_limit + 1` (дальше всё равно «вне сетки»).
pub(crate) fn guess_cell(numerator: f32, denominator: f32, size_limit: f32) -> f32 {
    let reciprocal = 1.0 / denominator;
    (numerator * reciprocal).floor().min(size_limit + 1.0)
}

/// Ячейка по оси: `None` — вне сетки (`t < 0`, не число или ячейка не меньше `size_limit`).
pub(crate) fn cell_on_axis(numerator: f32, denominator: f32, size_limit: f32) -> Option<u32> {
    if numerator.is_nan() || numerator < 0.0 {
        return None;
    }
    let guess = guess_cell(numerator, denominator, size_limit);
    let cell = settle_cell(guess, numerator, denominator);
    // Значение целое, неотрицательное (ноль не превышает `t ≥ 0`) и меньше предела: приведение безопасно.
    (cell < size_limit).then_some(cell as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Определение ячейки прямым перебором: наибольшее `c`, где `fl(c·S) ≤ t`.
    fn cell_by_definition(numerator: f32, denominator: f32, size_limit: f32) -> f32 {
        let mut cell = 0.0_f32;
        while cell < size_limit + 2.0 && (cell + 1.0) * denominator <= numerator {
            cell += 1.0;
        }
        cell
    }

    /// Простой генератор (xorshift): детерминированные «случайные» входы без внешних крейтов.
    struct Sequence(u64);
    impl Sequence {
        fn next_f32(&mut self, low: f32, high: f32) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            low + (high - low) * ((self.0 >> 40) as f32 / (1u64 << 24) as f32)
        }
    }

    #[test]
    fn settling_gives_the_defined_cell_from_every_guess_within_one_cell() {
        let mut random = Sequence(0x9E37_79B9_7F4A_7C15);
        let size = 512.0_f32;
        for _ in 0..20_000 {
            let denominator = random.next_f32(1e-3, 3.0);
            let numerator = random.next_f32(0.0, (size + 0.5) * denominator);
            let expected = cell_by_definition(numerator, denominator, size);
            for skew in [-1.0, 0.0, 1.0] {
                let guess = (expected + skew).max(0.0);
                assert_eq!(
                    settle_cell(guess, numerator, denominator),
                    expected,
                    "t={numerator} S={denominator} skew={skew}"
                );
            }
        }
    }

    #[test]
    fn boundary_numerators_land_on_the_cell_whose_product_they_equal_or_just_pass() {
        let mut random = Sequence(0x1234_5678_9ABC_DEF1);
        let size = 512.0_f32;
        for _ in 0..5_000 {
            let denominator = random.next_f32(1e-3, 3.0);
            let cell = (random.next_f32(1.0, size - 1.0)).floor();
            let boundary = cell * denominator;
            // Ровно на границе, на ULP ниже и на ULP выше.
            let below = f32::from_bits(boundary.to_bits() - 1);
            let above = f32::from_bits(boundary.to_bits() + 1);
            for numerator in [boundary, below, above] {
                let expected = cell_by_definition(numerator, denominator, size);
                let actual = cell_on_axis(numerator, denominator, size).map(|value| value as f32);
                assert_eq!(actual, Some(expected), "t={numerator} S={denominator}");
            }
            assert_eq!(cell_on_axis(boundary, denominator, size), Some(cell as u32));
            assert!(cell_on_axis(below, denominator, size).unwrap() < cell as u32);
        }
    }

    #[test]
    fn out_of_grid_inputs_have_no_cell() {
        assert_eq!(cell_on_axis(-1e-9, 1.0, 512.0), None);
        assert_eq!(cell_on_axis(f32::NAN, 1.0, 512.0), None);
        assert_eq!(cell_on_axis(512.0, 1.0, 512.0), None, "ровно size — уже вне сетки");
        assert_eq!(cell_on_axis(511.999, 1.0, 512.0), Some(511));
        assert_eq!(cell_on_axis(1e30, 1e-3, 512.0), None);
        assert_eq!(cell_on_axis(0.0, 1.0, 512.0), Some(0));
    }
}
