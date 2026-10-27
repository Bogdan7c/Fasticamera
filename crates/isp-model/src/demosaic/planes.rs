//! Плоскости f32 и свёртки с «зеркальной» границей — общий инструмент демозаика.
//!
//! Граничные правила повторяют эталон паритета (colour-demosaicing 0.2.7 / scipy), чтобы CPU-модель
//! и фикстуры совпадали вплоть до краёв кадра:
//! - одномерные свёртки — `scipy.ndimage.convolve1d(mode="mirror")`: отражение БЕЗ повтора
//!   крайнего отсчёта, `d c b | a b c d | c b a`;
//! - сдвиг для разности соседей — `np.pad(mode="reflect")`: то же самое зеркало;
//! - свёртка 5×5 классификатора — `scipy.ndimage.convolve(mode="constant")`: за краем нули.
//!
//! Все свёртки — настоящие свёртки (ядро переворачивается), как в scipy, а не корреляции.

/// Направление обработки: вдоль строк или вдоль столбцов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    /// Соседи слева и справа (вдоль строки).
    Horizontal,
    /// Соседи сверху и снизу (вдоль столбца).
    Vertical,
}

/// Одноканальная плоскость f32 в порядке строк.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Plane {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

/// Отражает индекс `index` в диапазон `0..length` по правилу «зеркало без повтора края».
///
/// Отражение периодично с периодом `2·(length − 1)`, поэтому правило верно и для кадров,
/// короче ядра (например, ширина 2 при ядре из 5 отсчётов). `length` ≥ 2 (мозаика чётная и непустая).
pub(super) fn mirror_index(index: isize, length: usize) -> usize {
    debug_assert!(length >= 2, "зеркало без повтора края требует минимум 2 отсчёта");
    // Размеры кадра заведомо влезают в isize (Vec не бывает больше isize::MAX байт).
    let period = 2 * (length as isize - 1);
    let folded = index.rem_euclid(period);
    // Вторая половина периода идёт «обратно»: ... c b | a b c d | c b a.
    if folded >= length as isize {
        (period - folded) as usize
    } else {
        folded as usize
    }
}

impl Plane {
    /// Плоскость, в которой отсчёт (x, y) задаёт функция.
    pub(super) fn from_fn(width: usize, height: usize, mut sample: impl FnMut(usize, usize) -> f32) -> Self {
        let mut values = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                values.push(sample(x, y));
            }
        }
        Self { width, height, values }
    }

    /// Плоскость из готового вектора. Длина обязана быть `width · height` (инвариант вызывающего).
    pub(super) fn from_values(width: usize, height: usize, values: Vec<f32>) -> Self {
        debug_assert_eq!(
            values.len(),
            width * height,
            "размер плоскости не совпал с числом отсчётов"
        );
        Self { width, height, values }
    }

    pub(super) fn width(&self) -> usize {
        self.width
    }

    /// Отсчёт (x, y) внутри кадра.
    pub(super) fn at(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < self.width && y < self.height, "координата вне плоскости");
        self.values[y * self.width + x]
    }

    /// Отсчёт со смещением `(dx, dy)` от (x, y); за краем действует зеркало без повтора края.
    pub(super) fn at_mirrored(&self, x: usize, y: usize, dx: isize, dy: isize) -> f32 {
        let mirrored_x = mirror_index(x as isize + dx, self.width);
        let mirrored_y = mirror_index(y as isize + dy, self.height);
        self.at(mirrored_x, mirrored_y)
    }

    /// Новая плоскость того же размера: значение в (x, y) считает функция от старой плоскости.
    pub(super) fn map_with_position(&self, mut transform: impl FnMut(usize, usize, f32) -> f32) -> Self {
        Self::from_fn(self.width, self.height, |x, y| transform(x, y, self.at(x, y)))
    }

    /// Поэлементная комбинация двух плоскостей одного размера: `combine(self, other)`.
    pub(super) fn zip_with(&self, other: &Plane, mut combine: impl FnMut(f32, f32) -> f32) -> Self {
        debug_assert!(
            self.width == other.width && self.height == other.height,
            "плоскости разного размера"
        );
        let values = self
            .values
            .iter()
            .zip(&other.values)
            .map(|(&left, &right)| combine(left, right))
            .collect();
        Self {
            width: self.width,
            height: self.height,
            values,
        }
    }

    /// Одномерная свёртка с зеркальной границей вдоль `direction`:
    /// `out[i] = Σ_j taps[j] · in[i + c − j]`, где `c = N / 2` (ядро переворачивается, как в scipy).
    pub(super) fn convolve_mirrored<const TAP_COUNT: usize>(
        &self,
        direction: Direction,
        taps: &[f32; TAP_COUNT],
    ) -> Self {
        let centre = (TAP_COUNT / 2) as isize;
        Self::from_fn(self.width, self.height, |x, y| {
            let mut sum = 0.0_f32;
            for (tap_index, &weight) in taps.iter().enumerate() {
                let offset = centre - tap_index as isize;
                let sample = match direction {
                    Direction::Horizontal => self.at_mirrored(x, y, offset, 0),
                    Direction::Vertical => self.at_mirrored(x, y, 0, offset),
                };
                sum += weight * sample;
            }
            sum
        })
    }

    /// Разность по модулю с соседом через один отсчёт вперёд: `|p[i] − p[i + 2]|`; у правого/нижнего
    /// края сосед берётся зеркально (`np.pad(mode="reflect")`, затем срез `[2:]`).
    pub(super) fn absolute_forward_difference(&self, direction: Direction) -> Self {
        const STEP: isize = 2;
        self.map_with_position(|x, y, value| {
            let neighbour = match direction {
                Direction::Horizontal => self.at_mirrored(x, y, STEP, 0),
                Direction::Vertical => self.at_mirrored(x, y, 0, STEP),
            };
            (value - neighbour).abs()
        })
    }

    /// Свёртка 5×5 с нулями за краем: `out[y][x] = Σ_{m,n} kernel[m][n] · in[y + 2 − m][x + 2 − n]`
    /// (`scipy.ndimage.convolve(mode="constant")`; ядро переворачивается, поэтому ориентация
    /// несимметричного ядра важна).
    pub(super) fn convolve_zero_padded_5x5(&self, kernel: &[[f32; 5]; 5]) -> Self {
        const CENTRE: isize = 2;
        Self::from_fn(self.width, self.height, |x, y| {
            let mut sum = 0.0_f32;
            for (row, kernel_row) in kernel.iter().enumerate() {
                for (column, &weight) in kernel_row.iter().enumerate() {
                    if weight == 0.0 {
                        continue;
                    }
                    let source_x = x as isize + CENTRE - column as isize;
                    let source_y = y as isize + CENTRE - row as isize;
                    // Вне кадра отсчёта нет — вклад нулевой (нулевое дополнение).
                    if source_x < 0
                        || source_y < 0
                        || source_x >= self.width as isize
                        || source_y >= self.height as isize
                    {
                        continue;
                    }
                    sum += weight * self.at(source_x as usize, source_y as usize);
                }
            }
            sum
        })
    }

    pub(super) fn as_slice(&self) -> &[f32] {
        &self.values
    }

    pub(super) fn into_values(self) -> Vec<f32> {
        self.values
    }
}

#[cfg(test)]
#[path = "planes_tests.rs"]
mod tests;
