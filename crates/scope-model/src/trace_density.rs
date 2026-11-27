//! Плотность следа на пиксель картинки: счётчики сетки, перенесённые в пиксели ядром [`AxisResampling`]
//! (решение 322). Эталон ДВУХ проходов GPU, по проходу на ось:
//! 1. по горизонтали (`scope_resample.comp`): для каждой строки сетки и каждого столбца пикселей — сумма соседних
//!    столбцов сетки с весами; итог — промежуточный слой «строка сетки × столбец картинки × слот»;
//! 2. по вертикали (`scope_render.comp`): для каждого пикселя — сумма соседних строк промежуточного слоя.
//!
//! Слот — плоскость, которую пиксель рисует. Наложение (RGB waveform) рисует все плоскости в каждом пикселе —
//! слотов столько, сколько плоскостей. Секции (parade) рисуют в пикселе только плоскость своей секции, а
//! вектороскоп — единственную плоскость, поэтому у них слот один.
//!
//! Секции: пиксель `x` принадлежит плоскости `x · planes / width` (целочисленно, как прежде); секция плоскости `k`
//! — пиксели `ceil(k · width / planes) .. ceil((k + 1) · width / planes)`, и сетка плоскости ложится на ширину
//! ИМЕННО этой секции (ядро не перетекает через границу секций).

use scope_contract::ScopeImageSize;

use crate::grid::ScopeCounters;
use crate::sample_count::SampleCount;
use crate::trace_paint::TracePaint;
use crate::trace_resample::AxisResampling;

/// Плоскость и границы секции, которой принадлежит пиксель `x` (только способ «секции»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    /// Плоскость секции.
    pub plane: u32,
    /// Первый пиксель секции.
    pub start: u32,
    /// Ширина секции, пикселей.
    pub width: u32,
}

impl Section {
    /// Секция плоскости `plane` картинки шириной `width` на `planes` секций; плоскость сверх `planes` берёт последнюю.
    pub fn of_plane(plane: u32, width: u32, planes: u32) -> Self {
        let planes = planes.max(1);
        let plane = plane.min(planes - 1);
        let start = section_start(plane, width, planes);
        Self {
            plane,
            start,
            width: section_start(plane + 1, width, planes) - start,
        }
    }

    /// Секция пикселя `x` картинки ширины `width`, поделённой на `planes` равных секций.
    pub fn of_pixel(x: u32, width: u32, planes: u32) -> Self {
        let planes = planes.max(1);
        let plane = ((u64::from(x) * u64::from(planes)) / u64::from(width.max(1))) as u32;
        let start = section_start(plane, width, planes);
        let end = section_start(plane + 1, width, planes);
        Self {
            plane,
            start,
            width: end - start,
        }
    }
}

/// Первый пиксель секции `plane`: `ceil(plane · width / planes)` — наименьший `x` с `x · planes / width ≥ plane`.
fn section_start(plane: u32, width: u32, planes: u32) -> u32 {
    (u64::from(plane) * u64::from(width)).div_ceil(u64::from(planes)) as u32
}

/// Сколько слотов плоскостей у пикселя при способе `paint` и `planes` плоскостях сетки.
pub fn slots_per_pixel(paint: TracePaint, planes: u32) -> u32 {
    match paint {
        TracePaint::PlanesOverlaid => planes,
        TracePaint::PlaneSections | TracePaint::ColorByPosition => 1,
    }
}

/// Попадания на пиксель по слотам: `[y][x][слот]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PictureDensity {
    size: ScopeImageSize,
    slots: u32,
    hits: Vec<f32>,
}

impl PictureDensity {
    /// Перенос счётчиков кадра с отсчётами `samples` в пиксели картинки `size` способом `paint` с мягкостью
    /// `softening_px`. Каждое попадание несёт массу своего блока прореживания (`SampleCount::pixels_per_sample`).
    pub fn compute(
        counters: &ScopeCounters,
        samples: SampleCount,
        paint: TracePaint,
        size: ScopeImageSize,
        softening_px: f32,
    ) -> Self {
        let resampled_columns = resample_columns(counters, paint, size, softening_px);
        let slots = slots_per_pixel(paint, counters.shape().planes()) as usize;
        let vertical = AxisResampling::new(counters.shape().rows(), size.height(), softening_px)
            .carrying_pixels_per_hit(samples.pixels_per_sample());
        let width = size.width() as usize;
        let mut hits = Vec::with_capacity(width * size.height() as usize * slots);
        for y in 0..size.height() {
            // Веса строк одинаковы для всех столбцов пикселя: считаются один раз на строку картинки.
            let taps: Vec<(usize, f32)> = vertical
                .cell_range(y)
                .map(|row| (row as usize, vertical.weight(row, y)))
                .collect();
            for x in 0..width {
                for slot in 0..slots {
                    let sum: f32 = taps
                        .iter()
                        .map(|&(row, weight)| resampled_columns[(row * width + x) * slots + slot] * weight)
                        .sum();
                    hits.push(sum);
                }
            }
        }
        Self {
            size,
            slots: slots as u32,
            hits,
        }
    }

    /// Попаданий на пиксель `(x, y)` в слоте `slot` (вне картинки — 0).
    pub fn hits(&self, x: u32, y: u32, slot: u32) -> f32 {
        if x >= self.size.width() || y >= self.size.height() || slot >= self.slots {
            return 0.0;
        }
        let index = (y as usize * self.size.width() as usize + x as usize) * self.slots as usize + slot as usize;
        self.hits[index]
    }

    /// Слотов у пикселя.
    pub fn slots(&self) -> u32 {
        self.slots
    }
}

/// Что читает столбец пикселей `x` на первом проходе: плоскость первого слота и веса столбцов сетки.
struct ColumnTaps {
    /// Плоскость слота 0 (у наложения слот `k` читает плоскость `k`).
    first_plane: u32,
    /// Столбцы сетки и их веса.
    taps: Vec<(u32, f32)>,
}

/// Веса столбцов сетки для каждого столбца пикселей (не зависят от строки сетки — считаются один раз).
fn column_taps(columns: u32, planes: u32, paint: TracePaint, width: u32, softening_px: f32) -> Vec<ColumnTaps> {
    let full_width = AxisResampling::new(columns, width, softening_px);
    // У секций ширины бывают двух видов (отличаются на пиксель): ядро на каждую плоскость строится один раз.
    let sections: Vec<AxisResampling> = (0..planes)
        .map(|plane| {
            let first_pixel = Section::of_pixel(0, width, planes);
            let section = (0..width)
                .map(|x| Section::of_pixel(x, width, planes))
                .find(|section| section.plane == plane)
                .unwrap_or(first_pixel);
            AxisResampling::new(columns, section.width, softening_px)
        })
        .collect();
    (0..width)
        .map(|x| {
            let (first_plane, horizontal, local_x) = match paint {
                TracePaint::PlaneSections => {
                    let section = Section::of_pixel(x, width, planes);
                    (section.plane, sections[section.plane as usize], x - section.start)
                }
                TracePaint::PlanesOverlaid | TracePaint::ColorByPosition => (0, full_width, x),
            };
            let taps = horizontal
                .cell_range(local_x)
                .map(|column| (column, horizontal.weight(column, local_x)))
                .collect();
            ColumnTaps { first_plane, taps }
        })
        .collect()
}

/// Первый проход: строки сетки × столбцы картинки × слоты (`[строка][x][слот]`).
fn resample_columns(counters: &ScopeCounters, paint: TracePaint, size: ScopeImageSize, softening_px: f32) -> Vec<f32> {
    let shape = counters.shape();
    let slots = slots_per_pixel(paint, shape.planes());
    let width = size.width();
    let per_column = column_taps(shape.columns(), shape.planes(), paint, width, softening_px);
    let mut resampled = Vec::with_capacity(shape.rows() as usize * width as usize * slots as usize);
    for row in 0..shape.rows() {
        for column in &per_column {
            for slot in 0..slots {
                let plane = column.first_plane + slot;
                let sum: f32 = column
                    .taps
                    .iter()
                    .map(|&(grid_column, weight)| counters.count(plane, row, grid_column).unwrap_or(0) as f32 * weight)
                    .sum();
                resampled.push(sum);
            }
        }
    }
    resampled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GridShape;

    #[test]
    fn sections_split_the_width_into_the_same_pixel_ranges_as_the_integer_plane_rule() {
        // 1011 px на 3 плоскости: 0..337, 337..674, 674..1011.
        for x in 0..1011 {
            let section = Section::of_pixel(x, 1011, 3);
            assert_eq!(section.plane, x * 3 / 1011);
            assert!(
                x >= section.start && x < section.start + section.width,
                "x {x}: {section:?}"
            );
        }
        assert_eq!(
            Section::of_pixel(0, 1011, 3).width
                + Section::of_pixel(500, 1011, 3).width
                + Section::of_pixel(1010, 1011, 3).width,
            1011
        );
    }

    #[test]
    fn section_of_a_plane_is_the_section_of_its_pixels() {
        for (width, planes) in [(1011, 3), (301, 2), (30, 3), (7, 3)] {
            for plane in 0..planes {
                let section = Section::of_plane(plane, width, planes);
                for x in section.start..section.start + section.width {
                    assert_eq!(Section::of_pixel(x, width, planes), section, "{width}/{planes}: x {x}");
                }
            }
        }
        // Плоскость сверх числа секций берёт последнюю секцию.
        assert_eq!(Section::of_plane(5, 1011, 3), Section::of_plane(2, 1011, 3));
    }

    #[test]
    fn density_keeps_the_hits_of_the_grid() {
        // Одна плоскость 40 × 30, всё в одну строку посередине: сумма попаданий картинки = сумма счётчиков.
        let shape = GridShape::try_new(40, 30, 1).unwrap();
        let mut counts = vec![0_u32; shape.cell_count()];
        for column in 0..40 {
            counts[15 * 40 + column] = 10;
        }
        let counters = ScopeCounters::from_counts(shape, counts).unwrap();
        let size = ScopeImageSize::try_new(97, 61).unwrap();
        let total_hits = |samples: SampleCount| -> f32 {
            let density = PictureDensity::compute(&counters, samples, TracePaint::ColorByPosition, size, 0.7);
            (0..61)
                .flat_map(|y| (0..97).map(move |x| (x, y)))
                .map(|(x, y)| density.hits(x, y, 0))
                .sum()
        };
        // На краях оси масса за краем теряется (там нет данных): проверяем, что внутренние строки донесли своё.
        let every_pixel = total_hits(SampleCount::for_window(40, 30, crate::SampleStride::EVERY_PIXEL));
        assert!((every_pixel - 400.0).abs() / 400.0 < 0.03, "{every_pixel}");
        // Отсчёты с шагом 1 × 2 (решение 329): каждое попадание стоит за 2 пикселя кадра — масса вдвое больше.
        let steps = crate::SampleSteps::new(
            crate::SampleStride::EVERY_PIXEL,
            crate::SampleStride::EVERY_SECOND_PIXEL,
        );
        let halved_rows = total_hits(SampleCount::for_window(40, 60, steps));
        assert!(
            (halved_rows / every_pixel - 2.0).abs() < 1e-5,
            "{halved_rows} против {every_pixel}"
        );
    }
}
