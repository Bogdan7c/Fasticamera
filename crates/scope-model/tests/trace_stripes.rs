//! Вертикальные полосы следа RGB waveform и parade: корень и защита.
//!
//! Корень: отсчёты по столбцам делятся неравномерно (портрет телефона: ширина глаза 1536, шаг 2 → 768 отсчётов на
//! 512 столбцов; 256 столбцов получают один отсчёт по горизонтали, 256 — два). Если порог полной яркости один на
//! все столбцы (среднее), «двойные» столбцы горят заметно ярче «одинарных» — полосы через столбец. Порог по СВОИМ
//! отсчётам столбца ([`ColumnBasis::OwnSamples`]) это убирает.
//!
//! Яркость лог-шкалы `log2(1 + count) / log2(1 + порог)` нелинейна: вдвое больший счётчик при вдвое большем пороге
//! даёт не ту же яркость, поэтому остаток контраста не нуль (его замеряет тест и держит ниже прежнего).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::picture;
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{
    ColumnBasis, RenderSettings, ResolvedScope, SampleCount, SampleStride, ScopeModelSettings, accumulate, render,
    render_with_basis, samples_in_columns,
};

const COLUMNS: u32 = 512;
const EYE_WIDTH: u32 = 1536;
const PICTURE_HEIGHT: u32 = 600;

/// Прибор waveform или parade с 512 столбцами и 256 уровнями (умолчания продукта).
fn resolve(kind: ScopeKind) -> ResolvedScope {
    let request = ScopeRequest {
        kind,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        scan: ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(COLUMNS, 64).unwrap(),
    };
    let settings = ScopeModelSettings {
        trace_columns: COLUMNS,
        trace_levels: 256,
        ..ScopeModelSettings::default()
    };
    ResolvedScope::resolve(&request, &settings, DisplayGamut::Srgb).unwrap()
}

/// Суммарная альфа следа по каждому столбцу: пиксель картинки `COLUMNS` шириной — ровно один столбец сетки; у
/// parade берётся секция первой плоскости (она занимает первую треть ширины картинки).
fn alpha_per_column(kind: ScopeKind, picture_codes: &scope_model::ShownCodes, basis: ColumnBasis) -> Vec<f32> {
    let scope = resolve(kind);
    let stride = SampleStride::EVERY_SECOND_PIXEL;
    let counters = accumulate(picture_codes, stride, &scope).unwrap();
    let samples = SampleCount::for_window(picture_codes.width(), picture_codes.height(), stride);
    let settings = RenderSettings {
        graticule_opacity: 0.0,
        ..RenderSettings::default()
    };
    // Parade: три секции по ширине, первая — плоскость R, её столбцы идут подряд: ширина картинки = 3 · столбцов.
    let width = if kind == ScopeKind::RgbParade {
        COLUMNS * 3
    } else {
        COLUMNS
    };
    let size = ScopeImageSize::try_new(width, 64).unwrap();
    // Продуктовый путь — `render` (по своим отсчётам столбца); прежнее среднее — только через явную основу.
    let rendered = match basis {
        ColumnBasis::OwnSamples => render(&counters, &scope, &settings, size, samples),
        ColumnBasis::WholeFrameAverage => render_with_basis(&counters, &scope, &settings, size, samples, basis),
    }
    .unwrap();
    (0..COLUMNS)
        .map(|column| (0..64).map(|y| rendered.pixel(column, y).unwrap()[3]).sum())
        .collect()
}

/// Отношение средней яркости «двойных» столбцов (больше отсчётов) к «одинарным».
fn double_to_single_contrast(alpha: &[f32]) -> f32 {
    let samples = samples_in_columns(
        SampleCount::for_window(EYE_WIDTH, PICTURE_HEIGHT, SampleStride::EVERY_SECOND_PIXEL),
        ScopeScan::Upright,
        COLUMNS,
    );
    let fewest = *samples.iter().min().unwrap();
    let mean_of = |wanted_double: bool| {
        let values: Vec<f32> = alpha
            .iter()
            .zip(&samples)
            .filter(|(_, count)| (**count > fewest) == wanted_double)
            .map(|(value, _)| *value)
            .collect();
        assert_eq!(
            values.len(),
            (COLUMNS / 2) as usize,
            "256 «одинарных» и 256 «двойных» столбцов"
        );
        values.iter().sum::<f32>() / values.len() as f32
    };
    mean_of(true) / mean_of(false)
}

/// Картинка, в каждом столбце которой одно и то же: уровень растёт сверху вниз (код зависит только от `y`), с
/// небольшой разбросанностью по каналам, чтобы три плоскости не совпадали целиком.
fn vertical_ramp() -> scope_model::ShownCodes {
    picture(EYE_WIDTH, PICTURE_HEIGHT, |_, y| {
        let level = (y * 1023 / (PICTURE_HEIGHT - 1)) as u16;
        [level, 1023 - level, level / 2 + 256]
    })
}

#[test]
fn per_column_reference_removes_most_of_the_every_other_column_stripes_of_waveform_and_parade() {
    let codes = vertical_ramp();
    for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
        let before = double_to_single_contrast(&alpha_per_column(kind, &codes, ColumnBasis::WholeFrameAverage));
        let after = double_to_single_contrast(&alpha_per_column(kind, &codes, ColumnBasis::OwnSamples));
        println!("{kind:?}: контраст «двойных» к «одинарным» столбцам: было {before:.3}, стало {after:.3}");
        // Прежнее среднее: «двойные» столбцы заметно ярче «одинарных» (полосы).
        assert!(
            before > 1.3,
            "{kind:?}: до исправления полосы должны быть заметны: {before}"
        );
        // Порог по своим отсчётам: полосы остаются лишь как остаток нелинейности лог-шкалы, вдвое слабее.
        assert!(
            after < 1.0 + (before - 1.0) * 0.5,
            "{kind:?}: было {before}, стало {after}"
        );
        assert!(after < 1.25, "{kind:?}: остаточный контраст {after}");
    }
}
