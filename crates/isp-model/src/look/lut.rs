//! Задел LUT (решения 99, 176): трёхмерная таблица `N³` узлов после цвета вида и до перевода в цвета выхода.
//!
//! # Домен
//! Вход и выход — линейный свет рабочего пространства `W` (после основы и цвета вида):
//! ```text
//! e = sRGB_OETF(clamp(rgb, 0, 1))      — по каналам; NaN → 0 ([`crate::transfer::srgb_encode`])
//! t = таблица(e)                       — тетраэдральная интерполяция (ниже)
//! out = sRGB_EOTF(clamp(t, 0, 1))      — обратно в линейный свет ([`crate::transfer::srgb_decode`])
//! ```
//! Значения вне `[0, 1]` обрезаются на входе (таблица определена только на кубе): при подключённой таблице
//! пересвет и отрицательный свет вне охвата W до неё теряются. Импортёр `.cube` (бэклог) обязан привести
//! таблицу к этому домену.
//!
//! # Раскладка узлов
//! `nodes[r + N·(g + N·b)]` — красный индекс бежит быстрее всех (как в формате `.cube`). Узел `(r, g, b)` — значение
//! таблицы при кодированном входе `(r, g, b)/(N − 1)`.
//!
//! # Тетраэдральная интерполяция
//! `p = e·(N − 1)`, `i = min(⌊p⌋, N − 2)` по каждой оси, `f = p − i ∈ [0, 1]`. Куб `[i, i+1]³` делится на 6 тетраэдров
//! по порядку дробных частей. Ось с наибольшей дробной частью — первая (`f₁ ≥ f₂ ≥ f₃`), путь от узла `V₀₀₀`
//! к `V₁₁₁` добавляет оси в этом порядке:
//! ```text
//! out = V₀ + f₁·(V₁ − V₀) + f₂·(V₂ − V₁) + f₃·(V₃ − V₂)
//! ```
//! где `V₀ = V(i)`, `V₁ = V₀ + шаг оси 1`, `V₂ = V₁ + шаг оси 2`, `V₃ = V(i + 1, 1, 1)`. Это 4 узла вместо 8 у трилинейной;
//! функция непрерывна на гранях тетраэдров (при равных дробных частях порядок осей не меняет результат) и точно
//! воспроизводит любую аффинную функцию узлов (тождество, перестановку каналов). При равенстве дробных частей
//! порядок осей — `R, G, B` (устойчивая сортировка); GPU обязан повторять его.

use std::sync::Arc;

use crate::transfer::{srgb_decode, srgb_encode};

/// Таблица не годится.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidLut {
    #[error("размер таблицы {size}: нужно от 2 до {max} узлов на ось")]
    SizeOutOfRange { size: u32, max: u32 },
    #[error("для размера {size} нужно {expected} узлов, передано {actual}")]
    NodeCountMismatch { size: u32, expected: usize, actual: usize },
    #[error("узел {index} содержит нечисловое значение (NaN или бесконечность)")]
    NonFiniteNode { index: usize },
}

/// Трёхмерная таблица цвета вида (см. модуль). Построить можно только конструктором.
#[derive(Debug, Clone, PartialEq)]
pub struct LookLut {
    size: u32,
    nodes: Vec<[f32; 3]>,
}

impl LookLut {
    /// Проверенная таблица: `2 ≤ size ≤ max_size`, `size³` конечных узлов. `max_size` — предел из конфига модели.
    pub fn new(size: u32, nodes: Vec<[f32; 3]>, max_size: u32) -> Result<Arc<Self>, InvalidLut> {
        if size < 2 || size > max_size {
            return Err(InvalidLut::SizeOutOfRange { size, max: max_size });
        }
        let expected = (size as usize).pow(3);
        if nodes.len() != expected {
            return Err(InvalidLut::NodeCountMismatch {
                size,
                expected,
                actual: nodes.len(),
            });
        }
        if let Some(index) = nodes
            .iter()
            .position(|node| node.iter().any(|value| !value.is_finite()))
        {
            return Err(InvalidLut::NonFiniteNode { index });
        }
        Ok(Arc::new(Self { size, nodes }))
    }

    /// Таблица-«единица» (узел = его координата): тождество в кодированном домене.
    pub fn identity(size: u32, max_size: u32) -> Result<Arc<Self>, InvalidLut> {
        let last = size.saturating_sub(1).max(1) as f32;
        let mut nodes = Vec::with_capacity((size as usize).pow(3));
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    nodes.push([red as f32 / last, green as f32 / last, blue as f32 / last]);
                }
            }
        }
        Self::new(size, nodes, max_size)
    }

    /// Узлов на ось.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Узлы в раскладке модуля (для упаковки в буфер GPU).
    pub fn nodes(&self) -> &[[f32; 3]] {
        &self.nodes
    }

    fn node(&self, red: usize, green: usize, blue: usize) -> [f32; 3] {
        let size = self.size as usize;
        self.nodes[red + size * (green + size * blue)]
    }

    /// Значение таблицы при кодированном входе `encoded ∈ [0, 1]³` (тетраэдральная интерполяция модуля).
    pub fn sample_encoded(&self, encoded: [f32; 3]) -> [f32; 3] {
        let last = (self.size - 1) as f32;
        let max_cell = (self.size - 2) as usize;
        let mut cell = [0_usize; 3];
        let mut fraction = [0.0_f32; 3];
        for axis in 0..3 {
            let position = encoded[axis].clamp(0.0, 1.0) * last;
            cell[axis] = (position.floor() as usize).min(max_cell);
            fraction[axis] = position - cell[axis] as f32;
        }
        // Порядок осей по убыванию дробной части; равные — в порядке R, G, B (сортировка устойчива).
        let mut order = [0_usize, 1, 2];
        order.sort_by(|left, right| fraction[*right].total_cmp(&fraction[*left]));
        let mut corner = cell;
        let mut previous = self.node(corner[0], corner[1], corner[2]);
        let mut result = previous;
        for axis in order {
            corner[axis] += 1;
            let next = self.node(corner[0], corner[1], corner[2]);
            for channel in 0..3 {
                result[channel] += fraction[axis] * (next[channel] - previous[channel]);
            }
            previous = next;
        }
        result
    }

    /// Применяет таблицу к свету `W` (домен — в доке модуля).
    pub fn apply(&self, working_rgb: [f32; 3]) -> [f32; 3] {
        self.sample_encoded(working_rgb.map(srgb_encode)).map(srgb_decode)
    }
}

#[cfg(test)]
#[path = "lut_tests.rs"]
mod tests;
