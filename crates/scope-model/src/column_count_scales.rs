//! Масштаб счётчика КАЖДОГО столбца следа waveform и parade: выравнивает яркость столбцов с разным числом отсчётов.
//!
//! Отсчёты по столбцам делятся НЕ поровну: столбец сетки отсчёта — `x · columns / width` (см.
//! [`crate::trace_rules`]), а отсчёты стоят через шаг `stride`. Телефон в портрете: ширина глаза 1536, шаг 2 → 768
//! отсчётов на 512 столбцов; 256 столбцов получают один отсчёт по горизонтали, 256 — два. Тот же однородный кадр
//! даёт «двойным» столбцам вдвое больший счётчик — на следе вертикальные полосы через столбец. Менять только
//! порог мало: яркость идёт по лог-шкале, и вдвое больший счётчик при вдвое большем пороге не даёт ту же яркость
//! (замер: контраст 1,48 → 1,12). Поэтому счётчик приводится к «среднему столбцу»:
//!
//! `scale(col) = среднее_отсчётов_на_столбец / отсчётов_в_столбце(col)`, где среднее — `всего_отсчётов / столбцов`,
//! а порог полной яркости один на кадр ([`TraceReference`], доля среднего столбца). Яркость:
//! `min(1, log2(1 + count · scale) / log2(1 + порог))`. ПОРЯДОК ОПЕРАЦИЙ (f32) один у эталона и шейдера:
//! `count · scale`, затем `1 + …`, затем `log2`, затем деление на `trace.x`. Масштаб `1.0` даёт прежнюю формулу
//! бит в бит (`x · 1 = x`), поэтому сверка с замороженной копией шейдера идёт с [`ColumnBasis::Uniform`].
//!
//! Отсчётов в столбце — число положений вдоль горизонтали ГЛАЗА (с учётом зеркала и поворота телефона, как у
//! [`SamplePosition::from_shown`]), попавших в столбец, умноженное на число отсчётов по другой оси. Столбец без
//! отсчётов получает масштаб `0.0` (его счётчик нулевой, ветка `count == 0` даёт нулевую альфу и без него).
//!
//! Один и тот же список ([`ColumnCountScales`]) считает и эталон ([`crate::render`]), и GPU-сторона (таблица в блоке
//! показа), поэтому масштабы у них один в один (`f32`).

use scope_contract::ScopeScan;

use crate::rule::SamplePosition;
use crate::sample_count::SampleCount;
use crate::trace_paint::TracePaint;
use crate::trace_rules::column_of_eye_position;

/// На какой основе считаются масштабы столбцов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnBasis {
    /// Масштаб по СВОИМ отсчётам каждого столбца (продуктовый способ; ровный след при неравномерной раздаче).
    OwnSamples,
    /// Масштаб `1.0` во всех столбцах (прежняя формула без выравнивания). Нужен только сверке с замороженной
    /// копией шейдера, которая масштаба не знает.
    Uniform,
}

/// Масштабы счётчиков по столбцам следа (по одному `f32` на столбец сетки).
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnCountScales {
    scales: Vec<f32>,
}

impl ColumnCountScales {
    /// Масштабы столбцов для способа `paint`.
    ///
    /// `samples` — отсчёты кадра (с окном и шагом), `scan` — как глаз читает картинку, `columns` — столбцов сетки.
    /// Способ «по положению» (вектороскоп) столбцов не знает: у него масштаб `1.0` везде, какой бы `basis` ни был.
    pub fn new(paint: TracePaint, basis: ColumnBasis, samples: SampleCount, scan: ScopeScan, columns: u32) -> Self {
        let columns = columns.max(1);
        let aligns_columns = match (paint, basis) {
            (TracePaint::PlaneSections | TracePaint::PlanesOverlaid, ColumnBasis::OwnSamples) => true,
            (TracePaint::PlaneSections | TracePaint::PlanesOverlaid, ColumnBasis::Uniform)
            | (TracePaint::ColorByPosition, ColumnBasis::OwnSamples | ColumnBasis::Uniform) => false,
        };
        if !aligns_columns {
            return Self {
                scales: vec![1.0; columns as usize],
            };
        }
        // Среднее отсчётов на столбец — то же число, из которого считается порог кадра (`TraceReference::for_paint`).
        let mean_per_column = samples.total() as f32 / columns as f32;
        let scales = samples_in_columns(samples, scan, columns)
            .into_iter()
            .map(|samples_in_column| {
                if samples_in_column == 0 {
                    0.0
                } else {
                    mean_per_column / samples_in_column as f32
                }
            })
            .collect();
        Self { scales }
    }

    /// Сколько столбцов в списке.
    pub fn len(&self) -> usize {
        self.scales.len()
    }

    /// Пуст ли список (не бывает: столбцов не меньше одного).
    pub fn is_empty(&self) -> bool {
        self.scales.is_empty()
    }

    /// Масштаб столбца; `None` — столбца вне сетки нет.
    pub fn of_column(&self, column: u32) -> Option<f32> {
        self.scales.get(column as usize).copied()
    }

    /// Масштабы по столбцам — ровно те f32, что уходят в таблицу шейдера.
    pub fn values(&self) -> &[f32] {
        &self.scales
    }
}

/// Сколько отсчётов кадра легло в каждый из `columns` столбцов (положение отсчёта — по горизонтали глаза).
/// Сумма по столбцам равна числу отсчётов кадра ([`SampleCount::total`]).
pub fn samples_in_columns(samples: SampleCount, scan: ScopeScan, columns: u32) -> Vec<u64> {
    let (window_width, window_height) = samples.window();
    let stride = samples.stride();
    // Горизонталь глаза идёт вдоль строк поверхности или вдоль её столбцов — по повороту телефона.
    let eye_runs_along_rows = scan.runs_along_surface_rows();
    let (eye_samples, other_axis_samples) = if eye_runs_along_rows {
        (samples.down(), samples.across())
    } else {
        (samples.across(), samples.down())
    };
    let mut per_column = vec![0_u64; columns.max(1) as usize];
    for eye_index in 0..eye_samples {
        let coordinate = stride.coordinate(eye_index);
        // Вторая координата безразлична: положение глаза берётся только по своей оси.
        let (x, y) = if eye_runs_along_rows {
            (0, coordinate)
        } else {
            (coordinate, 0)
        };
        let position = SamplePosition::from_shown(x, y, window_width, window_height, scan);
        let column = column_of_eye_position(position, columns.max(1)) as usize;
        if let Some(slot) = per_column.get_mut(column) {
            *slot += u64::from(other_axis_samples);
        }
    }
    per_column
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampling::SampleStride;

    fn window(width: u32, height: u32, stride: u32) -> SampleCount {
        SampleCount::for_window(width, height, SampleStride::try_new(stride).unwrap())
    }

    const ALL_SCANS: [ScopeScan; 4] = [
        ScopeScan::Upright,
        ScopeScan::RotatedClockwise,
        ScopeScan::UpsideDown,
        ScopeScan::RotatedCounterClockwise,
    ];

    #[test]
    fn samples_of_all_columns_add_up_to_the_frame_total_for_every_scan() {
        for scan in ALL_SCANS {
            for (width, height, stride, columns) in [(1536, 800, 2, 512), (1365, 700, 2, 256), (37, 91, 3, 20)] {
                let samples = window(width, height, stride);
                let per_column = samples_in_columns(samples, scan, columns);
                assert_eq!(per_column.len(), columns as usize);
                assert_eq!(
                    per_column.iter().sum::<u64>(),
                    samples.total(),
                    "{scan:?} {width}x{height} шаг {stride}"
                );
            }
        }
    }

    #[test]
    fn portrait_phone_columns_alternate_between_one_and_two_sample_columns() {
        // 1536 / шаг 2 = 768 отсчётов на 512 столбцов: 256 столбцов с одним, 256 с двумя; строк отсчётов 400.
        let per_column = samples_in_columns(window(1536, 800, 2), ScopeScan::Upright, 512);
        let single = per_column.iter().filter(|&&count| count == 400).count();
        let double = per_column.iter().filter(|&&count| count == 800).count();
        assert_eq!((single, double), (256, 256), "{per_column:?}");
    }

    #[test]
    fn rotated_scan_counts_samples_along_the_other_surface_axis() {
        // Поворот: горизонталь глаза идёт вдоль высоты поверхности (800), отсчётов по ней 400 на 512 столбцов,
        // по другой оси (ширина 1536) — 768 отсчётов на каждое положение.
        let per_column = samples_in_columns(window(1536, 800, 2), ScopeScan::RotatedClockwise, 512);
        assert!(
            per_column.iter().all(|&count| count == 0 || count == 768),
            "{per_column:?}"
        );
        assert_eq!(per_column.iter().filter(|&&count| count == 768).count(), 400);
        assert_eq!(per_column.iter().sum::<u64>(), 400 * 768);
    }

    #[test]
    fn reversed_scan_mirrors_the_distribution_of_the_upright_one() {
        let upright = samples_in_columns(window(1537, 10, 2), ScopeScan::Upright, 512);
        let upside_down = samples_in_columns(window(1537, 10, 2), ScopeScan::UpsideDown, 512);
        let mut mirrored = upright.clone();
        mirrored.reverse();
        // Не равенство «столбец в столбец», а то же РАСПРЕДЕЛЕНИЕ: переворот меняет положения отсчётов, но не их число.
        let mut left = mirrored;
        let mut right = upside_down;
        left.sort_unstable();
        right.sort_unstable();
        assert_eq!(left, right);
    }

    #[test]
    fn scales_even_out_the_columns_to_the_mean_and_equalise_scaled_counts() {
        let samples = window(1536, 800, 2);
        let scales = ColumnCountScales::new(
            TracePaint::PlanesOverlaid,
            ColumnBasis::OwnSamples,
            samples,
            ScopeScan::Upright,
            512,
        );
        let per_column = samples_in_columns(samples, ScopeScan::Upright, 512);
        assert_eq!(scales.len(), 512);
        let mean = samples.total() as f32 / 512.0;
        for (column, &count) in per_column.iter().enumerate() {
            let scale = scales.of_column(column as u32).unwrap();
            assert_eq!(scale, mean / count as f32, "столбец {column}");
            // Одинаковая плотность попаданий в любом столбце даёт один и тот же приведённый счётчик.
            let scaled_full_column = count as f32 * scale;
            assert!((scaled_full_column - mean).abs() <= mean * 1e-6, "столбец {column}");
        }
        // «Двойные» столбцы получают вдвое меньший масштаб, чем «одинарные».
        assert_eq!(scales.of_column(0).unwrap() * 2.0, scales.of_column(1).unwrap());
    }

    #[test]
    fn a_column_without_samples_gets_scale_zero_and_outside_columns_are_absent() {
        let samples = window(8, 4, 1);
        let scales = ColumnCountScales::new(
            TracePaint::PlaneSections,
            ColumnBasis::OwnSamples,
            samples,
            ScopeScan::Upright,
            32,
        );
        let per_column = samples_in_columns(samples, ScopeScan::Upright, 32);
        let empty = per_column.iter().position(|&count| count == 0).unwrap();
        assert_eq!(scales.of_column(empty as u32), Some(0.0));
        assert_eq!(scales.of_column(32), None);
    }

    #[test]
    fn uniform_basis_and_position_paint_have_scale_one_in_every_column() {
        let samples = window(1536, 800, 2);
        for (paint, basis) in [
            (TracePaint::PlanesOverlaid, ColumnBasis::Uniform),
            (TracePaint::PlaneSections, ColumnBasis::Uniform),
            (TracePaint::ColorByPosition, ColumnBasis::OwnSamples),
            (TracePaint::ColorByPosition, ColumnBasis::Uniform),
        ] {
            let scales = ColumnCountScales::new(paint, basis, samples, ScopeScan::Upright, 64);
            assert_eq!(scales.len(), 64);
            assert!(scales.values().iter().all(|&scale| scale == 1.0), "{paint:?} {basis:?}");
        }
    }
}
