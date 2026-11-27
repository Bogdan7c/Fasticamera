//! Перенос сетки счётчиков в пиксели картинки прибора — плавно, без блоков и муара (решение 322).
//!
//! # Зачем
//! До 14r пиксель картинки брал ОДНУ ячейку сетки (ближайшую): сетка 512 × 256 на картинке 1011 × 827 рисовалась
//! блоками 2 × 3 px, а у каждого блока свой случайный счёт в каждом канале — цветное «конфетти» вместо следа.
//! Теперь пиксель собирает взвешенную сумму соседних ячеек: так рисуют следы аналоговые приборы (луч с
//! конечной толщиной), а «песок» сменяется плотностью.
//!
//! # Формула (одна ось; две оси — по очереди, ядро разделимо)
//! Ось сетки из `cells` клеток ложится на ось картинки из `pixels` пикселей; клетка занимает `ppc = pixels / cells`
//! пикселей. Расстояние от центра пикселя `p` до центра клетки `i` в пикселях:
//! `d = (i + 0,5) · ppc − (p + 0,5)`. Вес клетки — гауссиана
//! `w = exp(−d² / (2σ²)) / (σ · √(2π))`, где `σ² = s² + (k · max(ppc, 1))²`:
//! * `s` — мягкость следа из настроек (`trace_softening_px`, пиксели картинки);
//! * `k · max(ppc, 1)` — наименьшая ширина ядра, при которой сумма гауссиан не рябит: ядро не уже [`RIPPLE_FREE_SPAN`]
//!   шага сетки (при увеличении, ppc > 1) и не уже столько же пикселей (при уменьшении). Сумма сдвинутых гауссиан
//!   с шагом `Δ` рябит с относительной амплитудой ≈ `2 · exp(−2π² σ² / Δ²)`; при `σ ≥ 0,55 Δ` это < 0,5 %. Без этой
//!   доли при зуме 2× (шаг сетки 2,2 px) через клетку проступала бы рябь ≈ 4 % — те же полосы, только мельче.
//!
//! Сумма весов одной клетки по всем пикселям ≈ 1 (та же оценка ряби по шагу пикселей), поэтому перенос СОХРАНЯЕТ
//! число попаданий: счёт пикселя — это «сколько попаданий легло на этот пиксель». Берутся клетки с `|d| ≤ 3,5σ`:
//! за ними вес < 0,22 % пика, а теряемая масса < 0,05 %. У края оси клеток нет — масса за краем не добирается,
//! это честно: за краем нет данных.
//!
//! Эта же формула стоит в шейдерах `scope_resample.comp` (по горизонтали) и `scope_render.comp` (по вертикали);
//! порядок операций f32 один и тот же, итог сверяется с допуском (у `exp` на GPU своя точность).

use std::f32::consts::PI;

/// Сколько сигм ядра берётся с каждой стороны: дальше вес < 0,22 % пика, теряемая масса < 0,05 %.
pub const KERNEL_RADIUS_IN_SIGMAS: f32 = 3.5;

/// Наименьшая ширина ядра (сигма) в долях большего из шагов — сетки или пикселя: рябь суммы гауссиан < 0,5 %.
pub const RIPPLE_FREE_SPAN: f32 = 0.55;

/// Наибольшая мягкость следа, пикселей: дальше след расплывается в облако, а число клеток на пиксель растёт.
pub const MAX_TRACE_SOFTENING_PX: f32 = 4.0;

/// Перенос одной оси сетки (`cells` клеток) в ось картинки (`pixels` пикселей).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisResampling {
    cells: u32,
    pixels_per_cell: f32,
    /// `1 / (2σ²)` — множитель в показателе экспоненты.
    exponent_scale: f32,
    /// `1 / (σ · √(2π))` — высота гауссианы.
    peak: f32,
    /// Радиус ядра в пикселях: `3,5σ`.
    radius_px: f32,
}

impl AxisResampling {
    /// Перенос оси из `cells` клеток в `pixels` пикселей с мягкостью `softening_px` (пиксели картинки, ≥ 0).
    /// Нулевые `cells` или `pixels` трактуются как один (пустая ось не делит на ноль).
    pub fn new(cells: u32, pixels: u32, softening_px: f32) -> Self {
        let cells = cells.max(1);
        let pixels_per_cell = pixels.max(1) as f32 / cells as f32;
        let softening = softening_px.max(0.0);
        // Наименьшая ширина без ряби: доля большего из шагов — шага сетки (увеличение) или пикселя (уменьшение).
        let ripple_free = RIPPLE_FREE_SPAN * pixels_per_cell.max(1.0);
        let variance = softening * softening + ripple_free * ripple_free;
        let sigma = variance.sqrt();
        Self {
            cells,
            pixels_per_cell,
            exponent_scale: 1.0 / (2.0 * variance),
            peak: 1.0 / (sigma * (2.0 * PI).sqrt()),
            radius_px: KERNEL_RADIUS_IN_SIGMAS * sigma,
        }
    }

    /// Пикселей на клетку (`pixels / cells`).
    pub fn pixels_per_cell(&self) -> f32 {
        self.pixels_per_cell
    }

    /// Радиус ядра, пикселей (`3,5σ`).
    pub fn radius_px(&self) -> f32 {
        self.radius_px
    }

    /// Множитель показателя экспоненты `1 / (2σ²)` — ровно тот f32, что уходит в шейдер.
    pub fn exponent_scale(&self) -> f32 {
        self.exponent_scale
    }

    /// Высота гауссианы `1 / (σ · √(2π))` — ровно тот f32, что уходит в шейдер.
    pub fn peak(&self) -> f32 {
        self.peak
    }

    /// То же ядро, но каждое попадание несёт массу `pixels_per_hit` пикселей кадра (решение 329): отсчёт, взятый с
    /// шагом, стоит за весь свой блок `шаг_x × шаг_y`. Тогда счёт пикселя картинки в среднем тот же, что при каждом
    /// пикселе, и яркость следа (`log2(1 + счёт)`, её «+1» не масштабируется) не тускнеет от прореживания — растёт
    /// только шум. Масса вносится в высоту гауссианы ОДНИМ умножением f32: эталон и шейдер берут тот же `peak`, при
    /// массе 1 число не меняется бит в бит. Применяется к ОДНОЙ оси переноса (вертикали), иначе масса удвоилась бы.
    pub fn carrying_pixels_per_hit(self, pixels_per_hit: u32) -> Self {
        Self {
            peak: self.peak * pixels_per_hit.max(1) as f32,
            ..self
        }
    }

    /// Клетки, которые видит пиксель `pixel`: полуоткрытый диапазон `first..end` в пределах `0..cells`.
    pub fn cell_range(&self, pixel: u32) -> std::ops::Range<u32> {
        let centre = pixel as f32 + 0.5;
        // Центр клетки i — в (i + 0,5) · ppc; клетка нужна, если |центр клетки − центр пикселя| ≤ радиус.
        let lowest = ((centre - self.radius_px) / self.pixels_per_cell - 0.5).ceil();
        let highest = ((centre + self.radius_px) / self.pixels_per_cell - 0.5).floor();
        let first = lowest.max(0.0) as u32;
        let end = (highest + 1.0).clamp(0.0, self.cells as f32) as u32;
        first..end.max(first)
    }

    /// Вес клетки `cell` для пикселя `pixel` (в «долях попадания на пиксель»).
    pub fn weight(&self, cell: u32, pixel: u32) -> f32 {
        let distance = (cell as f32 + 0.5) * self.pixels_per_cell - (pixel as f32 + 0.5);
        self.peak * (-(distance * distance) * self.exponent_scale).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Сумма весов клетки по всем пикселям оси (сколько её попаданий доходит до картинки).
    fn delivered_mass(resampling: &AxisResampling, cell: u32, pixels: u32) -> f32 {
        (0..pixels)
            .filter(|&pixel| resampling.cell_range(pixel).contains(&cell))
            .map(|pixel| resampling.weight(cell, pixel))
            .sum()
    }

    #[test]
    fn every_inner_cell_delivers_its_hits_to_the_picture_whatever_the_ratio() {
        // Раздел на телефоне: 917 столбцов → 1011 px (увеличение), 512 уровней → 827 px; альбом: 2048 → 1251
        // (уменьшение), секция parade 2048 → 417. Масса каждой внутренней клетки ≈ 1 — нет «тёмных» и «двойных»
        // столбцов ни при каком соотношении.
        for (cells, pixels) in [
            (917, 1011),
            (512, 827),
            (2048, 1251),
            (2048, 417),
            (512, 512),
            (100, 1000),
        ] {
            let resampling = AxisResampling::new(cells, pixels, 0.7);
            for cell in (cells / 4)..(3 * cells / 4) {
                let mass = delivered_mass(&resampling, cell, pixels);
                assert!((mass - 1.0).abs() < 2e-3, "{cells}→{pixels}: клетка {cell} даёт {mass}");
            }
        }
    }

    #[test]
    fn a_uniform_grid_paints_a_flat_picture_without_periodic_stripes() {
        // Равномерная сетка (каждая клетка = 1 попадание) должна дать ровную картинку. Именно здесь ломался
        // прежний перенос «ближайшая клетка»: 917 клеток на 1011 px → у части пикселей два раза одна и та же клетка.
        let (cells, pixels) = (917_u32, 1011_u32);
        let resampling = AxisResampling::new(cells, pixels, 0.7);
        let expected = cells as f32 / pixels as f32;
        let margin = 8;
        for pixel in margin..pixels - margin {
            let sum: f32 = resampling
                .cell_range(pixel)
                .map(|cell| resampling.weight(cell, pixel))
                .sum();
            assert!(
                (sum - expected).abs() < 2e-3 * expected,
                "пиксель {pixel}: {sum} при ожидаемых {expected}"
            );
        }
    }

    #[test]
    fn range_covers_exactly_the_cells_within_the_kernel_radius_and_stays_inside_the_grid() {
        let resampling = AxisResampling::new(64, 200, 1.0);
        for pixel in 0..200 {
            let range = resampling.cell_range(pixel);
            assert!(range.end <= 64);
            for cell in 0..64 {
                let distance = ((cell as f32 + 0.5) * resampling.pixels_per_cell() - (pixel as f32 + 0.5)).abs();
                let inside = distance <= resampling.radius_px();
                assert_eq!(
                    range.contains(&cell),
                    inside,
                    "пиксель {pixel}, клетка {cell}, расстояние {distance}"
                );
            }
        }
    }

    #[test]
    fn softening_widens_the_kernel_and_degenerate_axes_do_not_divide_by_zero() {
        let sharp = AxisResampling::new(512, 827, 0.0);
        let soft = AxisResampling::new(512, 827, 2.0);
        assert!(soft.radius_px() > sharp.radius_px());
        let empty = AxisResampling::new(0, 0, 0.7);
        assert!(empty.weight(0, 0).is_finite());
        assert_eq!(empty.cell_range(0), 0..1);
    }
}
