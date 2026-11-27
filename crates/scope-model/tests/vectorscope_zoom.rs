//! Зум ×2 вектороскопа (решение 325): слабый цвет вдвое дальше от центра, сетка та же, цели за краем не рисуются.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::flat;
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes, VectorscopeZoom};
use scope_model::{
    GraticulePrimitive, ResolvedScope, SampleCount, SampleStride, ScopeModelSettings, ScopeRule, accumulate,
};

fn vectorscope(axes: VectorscopeAxes, zoom: VectorscopeZoom) -> ResolvedScope {
    let request = ScopeRequest {
        kind: ScopeKind::Vectorscope,
        vectorscope_axes: axes,
        vectorscope_zoom: zoom,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(64, 64).unwrap(),
    };
    ResolvedScope::resolve(&request, &ScopeModelSettings::default(), DisplayGamut::DisplayP3).unwrap()
}

/// Расстояние (в клетках) от центра сетки до клетки, куда легла заплатка кода `code`.
fn distance_from_centre(scope: &ResolvedScope, code: [u16; 3]) -> f64 {
    let counters = accumulate(&flat(2, 2, code), SampleStride::EVERY_PIXEL, scope).unwrap();
    let shape = counters.shape();
    let centre = f64::from(shape.columns()) / 2.0;
    let (row, column) = (0..shape.rows())
        .flat_map(|row| (0..shape.columns()).map(move |column| (row, column)))
        .find(|&(row, column)| counters.count(0, row, column).unwrap() > 0)
        .expect("заплатка попала в сетку");
    let dx = f64::from(column) + 0.5 - centre;
    let dy = f64::from(row) + 0.5 - centre;
    (dx * dx + dy * dy).sqrt()
}

#[test]
fn a_weak_cast_lands_twice_as_far_from_the_centre_at_double_zoom_on_both_axes() {
    // Слабо тёплый серый (перекос белого): у края при 1× он почти в центре.
    let weak_cast = [560, 512, 470];
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let normal = vectorscope(axes, VectorscopeZoom::Normal);
        let double = vectorscope(axes, VectorscopeZoom::Double);
        let (near, far) = (
            distance_from_centre(&normal, weak_cast),
            distance_from_centre(&double, weak_cast),
        );
        assert!(near > 3.0, "{axes:?}: перекос виден и при 1× ({near} клеток)");
        // Допуск — квантование клетки (±0,5 клетки на каждом из расстояний).
        assert!(
            (far / near - 2.0).abs() < 0.15,
            "{axes:?}: вдвое дальше: {near} → {far}"
        );
        // Зум — только числа правила: форма сетки (и ресурсы GPU) та же.
        let frame = SampleCount::for_window(2, 2, SampleStride::EVERY_PIXEL);
        assert_eq!(normal.shape(frame), double.shape(frame), "{axes:?}");
    }
}

#[test]
fn targets_beyond_the_edge_at_double_zoom_are_not_drawn_and_the_circle_stays() {
    let settings = ScopeModelSettings::default();
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let boxes = |zoom| {
            vectorscope(axes, zoom)
                .graticule(&settings.render)
                .iter()
                .filter(|primitive| matches!(primitive, GraticulePrimitive::BoxOutline { .. }))
                .count()
        };
        assert_eq!(boxes(VectorscopeZoom::Normal), 12, "{axes:?}: при 1× все цели");
        let zoomed = boxes(VectorscopeZoom::Double);
        assert!(zoomed < 12, "{axes:?}: при 2× часть целей за краем: {zoomed}");
        let rings = vectorscope(axes, VectorscopeZoom::Double)
            .graticule(&settings.render)
            .iter()
            .filter(|primitive| matches!(primitive, GraticulePrimitive::Ring { .. }))
            .count();
        assert_eq!(rings, 1, "{axes:?}: граница круга остаётся и при 2×");
    }
}
