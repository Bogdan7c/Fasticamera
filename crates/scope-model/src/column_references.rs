//! Опорное число яркости КАЖДОГО столбца следа waveform и parade.
//!
//! Полная яркость следа — доля отсчётов ОДНОГО столбца (решение 266). Но отсчёты по столбцам делятся НЕ поровну:
//! столбец сетки отсчёта — `x · columns / width` (см. [`crate::trace_rules`]), а отсчёты стоят через шаг `stride`.
//! Телефон в портрете: ширина глаза 1536, шаг 2 → 768 отсчётов на 512 столбцов; одни столбцы получают один отсчёт
//! по горизонтали, другие два. Если брать среднее на столбец (`отсчётов / столбцов`), «двойные» столбцы горят вдвое
//! ярче «одинарных» — на следе вертикальные полосы через столбец. Поэтому порог считается по СВОИМ отсчётам
//! каждого столбца: `max(1, доля · отсчётов_в_столбце)`, и серая картинка даёт ровный след.
//!
//! Отсчётов в столбце — это число положений вдоль горизонтали ГЛАЗА (с учётом зеркала и поворота телефона, как у
//! [`SamplePosition::from_shown`]), попавших в столбец, умноженное на число отсчётов по другой оси (каждое положение
//! вдоль глаза повторяется на каждой строке отсчётов). Столбец без отсчётов не горит никогда (его счётчик нулевой),
//! знаменатель у него — наименьший допустимый (порог 1).
//!
//! Один и тот же список ([`ColumnReferences`]) считает и эталон ([`crate::render`]), и GPU-сторона (таблица в блоке
//! показа), поэтому знаменатели у них один в один (`f32`).

use scope_contract::ScopeScan;

use crate::render_settings::RenderSettings;
use crate::rule::SamplePosition;
use crate::sample_count::SampleCount;
use crate::trace_paint::TracePaint;
use crate::trace_reference::TraceReference;
use crate::trace_rules::column_of_eye_position;

/// На какой основе считается опорное число столбцов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnBasis {
    /// Каждый столбец — по СВОИМ отсчётам (продуктовый способ; ровный след при неравномерной раздаче отсчётов).
    OwnSamples,
    /// Все столбцы — по среднему на столбец (прежний способ: `TraceReference::for_paint`). Нужен только сверке
    /// с замороженной копией шейдера, которая знает одно число на кадр (`trace.x`).
    WholeFrameAverage,
}

/// Опорное число яркости по столбцам следа (по одному на столбец сетки).
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnReferences {
    references: Vec<TraceReference>,
}

impl ColumnReferences {
    /// Опорные числа столбцов для способа `paint`.
    ///
    /// `samples` — отсчёты кадра (с окном и шагом), `scan` — как глаз читает картинку, `columns` — столбцов сетки.
    /// Способ «по положению» (вектороскоп) столбцов не знает: у него одно опорное число на кадр во всех столбцах,
    /// какой бы `basis` ни был.
    pub fn new(
        paint: TracePaint,
        basis: ColumnBasis,
        settings: &RenderSettings,
        samples: SampleCount,
        scan: ScopeScan,
        columns: u32,
    ) -> Self {
        let per_column = match (paint, basis) {
            (TracePaint::PlaneSections | TracePaint::PlanesOverlaid, ColumnBasis::OwnSamples) => true,
            (TracePaint::PlaneSections | TracePaint::PlanesOverlaid, ColumnBasis::WholeFrameAverage)
            | (TracePaint::ColorByPosition, ColumnBasis::OwnSamples | ColumnBasis::WholeFrameAverage) => false,
        };
        if per_column {
            let share = settings.trace_full_share_of_column.get();
            let references = samples_in_columns(samples, scan, columns)
                .into_iter()
                .map(|samples_in_column| TraceReference::for_column(share, samples_in_column))
                .collect();
            return Self { references };
        }
        let frame_reference = TraceReference::for_paint(paint, settings, samples, columns);
        Self {
            references: vec![frame_reference; columns.max(1) as usize],
        }
    }

    /// Сколько столбцов в списке.
    pub fn len(&self) -> usize {
        self.references.len()
    }

    /// Пуст ли список (не бывает: столбцов не меньше одного).
    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }

    /// Опорное число столбца; `None` — столбца вне сетки нет.
    pub fn of_column(&self, column: u32) -> Option<TraceReference> {
        self.references.get(column as usize).copied()
    }

    /// Знаменатели лог-шкалы `log2(1 + порог)` по столбцам — ровно те f32, что уходят в таблицу шейдера.
    pub fn log_denominators(&self) -> impl Iterator<Item = f32> + '_ {
        self.references.iter().map(|reference| reference.log_denominator())
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
    fn each_column_threshold_follows_its_own_samples_and_the_old_average_does_not() {
        let settings = RenderSettings::default();
        let samples = window(1536, 800, 2);
        let own = ColumnReferences::new(
            TracePaint::PlanesOverlaid,
            ColumnBasis::OwnSamples,
            &settings,
            samples,
            ScopeScan::Upright,
            512,
        );
        let average = ColumnReferences::new(
            TracePaint::PlanesOverlaid,
            ColumnBasis::WholeFrameAverage,
            &settings,
            samples,
            ScopeScan::Upright,
            512,
        );
        assert_eq!((own.len(), average.len()), (512, 512));
        let per_column = samples_in_columns(samples, ScopeScan::Upright, 512);
        let share = settings.trace_full_share_of_column.get();
        for (column, &count) in per_column.iter().enumerate() {
            let threshold = own.of_column(column as u32).unwrap().full_threshold();
            assert_eq!(threshold, (share * count as f32).max(1.0), "столбец {column}");
        }
        // Прежнее среднее одно на все столбцы: у «одинарных» и «двойных» столбцов порог был один и тот же.
        let first = average.of_column(0).unwrap();
        assert!((0..512).all(|column| average.of_column(column) == Some(first)));
        assert_ne!(
            own.of_column(0).unwrap().full_threshold(),
            own.of_column(1).unwrap().full_threshold(),
            "в одном из соседних столбцов отсчётов вдвое больше"
        );
    }

    #[test]
    fn a_column_without_samples_gets_the_smallest_threshold_and_outside_columns_are_absent() {
        let settings = RenderSettings::default();
        // Окно уже ширины сетки: часть столбцов пуста.
        let samples = window(8, 4, 1);
        let references = ColumnReferences::new(
            TracePaint::PlaneSections,
            ColumnBasis::OwnSamples,
            &settings,
            samples,
            ScopeScan::Upright,
            32,
        );
        let per_column = samples_in_columns(samples, ScopeScan::Upright, 32);
        let empty = per_column.iter().position(|&count| count == 0).unwrap();
        assert_eq!(references.of_column(empty as u32).unwrap().full_threshold(), 1.0);
        assert_eq!(references.of_column(32), None);
    }

    #[test]
    fn position_paint_has_one_reference_for_all_columns_whatever_the_basis() {
        let settings = RenderSettings::default();
        let samples = window(1536, 800, 2);
        let expected = TraceReference::for_paint(TracePaint::ColorByPosition, &settings, samples, 64);
        for basis in [ColumnBasis::OwnSamples, ColumnBasis::WholeFrameAverage] {
            let references = ColumnReferences::new(
                TracePaint::ColorByPosition,
                basis,
                &settings,
                samples,
                ScopeScan::Upright,
                64,
            );
            assert!(
                references
                    .log_denominators()
                    .all(|value| value == expected.log_denominator())
            );
            assert_eq!(references.len(), 64);
        }
    }
}
