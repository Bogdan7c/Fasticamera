//! Сетка гистограммы (решение 338): своя, более тусклая непрозрачность, на которую общая настройка сетки не влияет.
//!
//! Тест идёт через настоящий путь продукта: запрос → решённый прибор → план отсчётов → накопление → картинка.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::flat;
use isp_model::image::DisplayGamut;
use scope_contract::{HistogramMode, ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{ResolvedScope, ScopeModelSettings, ScopePicture, ShownCodes, TracePaint, accumulate, render};

fn size(width: u32, height: u32) -> ScopeImageSize {
    ScopeImageSize::try_new(width, height).unwrap()
}

/// Картинка гистограммы кадра `shown` размера `picture_size`.
fn picture_of(
    shown: &ShownCodes,
    mode: HistogramMode,
    settings: &ScopeModelSettings,
    picture_size: ScopeImageSize,
) -> ScopePicture {
    let request = ScopeRequest {
        kind: ScopeKind::Histogram,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: mode,
        scan: ScopeScan::Upright,
        image_size: picture_size,
    };
    let scope = ResolvedScope::resolve(&request, settings, DisplayGamut::Srgb).unwrap();
    let samples = scope.planned_samples(shown.width(), shown.height(), settings);
    let counters = accumulate(shown, samples.steps(), &scope).unwrap();
    render(&counters, &scope, &settings.render, picture_size, samples).unwrap()
}

/// Самая плотная сетка верхней строки картинки в средней половине ширины: у чёрного кадра столбик стоит у левого края,
/// поэтому здесь видна только сетка (линия 50 % толще прочих — её центр покрыт полностью).
fn top_row_grid_alpha(picture: &ScopePicture) -> f32 {
    let width = picture.size().width();
    (width / 4..width * 3 / 4)
        .map(|x| picture.pixel(x, 0).unwrap()[3])
        .fold(0.0_f32, f32::max)
}

#[test]
fn the_histogram_grid_has_its_own_dimmer_opacity_and_the_shared_one_does_not_touch_it() {
    let black = flat(64, 64, [0, 0, 0]);
    let picture_size = size(400, 200);
    let defaults = ScopeModelSettings::default();
    // Решение 338: у гистограммы сетка 0,2, у остальных приборов — общая 0,45.
    assert_eq!(defaults.render.histogram_graticule_opacity, 0.2);
    let drawn = picture_of(&black, HistogramMode::Rgb, &defaults, picture_size);
    assert!(
        (top_row_grid_alpha(&drawn) - 0.2).abs() < 1e-6,
        "{}",
        top_row_grid_alpha(&drawn)
    );
    // Общая непрозрачность сетки гистограмму не меняет.
    let mut shared_changed = defaults.clone();
    shared_changed.render.graticule_opacity = 0.9;
    let unchanged = picture_of(&black, HistogramMode::Rgb, &shared_changed, picture_size);
    assert_eq!(unchanged.pixels(), drawn.pixels());
    // А своя — меняет.
    let mut own_changed = defaults.clone();
    own_changed.render.histogram_graticule_opacity = 0.5;
    let brighter = picture_of(&black, HistogramMode::Rgb, &own_changed, picture_size);
    assert!((top_row_grid_alpha(&brighter) - 0.5).abs() < 1e-6);
    // Прочие способы рисования берут общую.
    assert_eq!(
        defaults.render.graticule_opacity_for(TracePaint::PlanesOverlaid),
        defaults.render.graticule_opacity
    );
    assert_eq!(
        defaults.render.graticule_opacity_for(TracePaint::HistogramSections),
        0.2
    );
}
