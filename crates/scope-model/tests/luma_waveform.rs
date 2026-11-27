//! Яркостный waveform (решение 321): одна плоскость Y' = 0,2126 R' + 0,7152 G' + 0,0722 B' по кодам сигнала.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{flat, picture};
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::trace_rules::{LumaRule, ParadeRule};
use scope_model::{
    RenderSettings, ResolvedScope, SampleCount, SampleStride, ScopeModelSettings, ScopeRule, TracePaint, accumulate,
    render,
};

/// Строка, куда правило кладёт код (один отсчёт картинки 1 × 1).
fn row_of(rule: &dyn ScopeRule, code: [u16; 3], plane: u32) -> u32 {
    let counters = accumulate(&flat(1, 1, code), SampleStride::EVERY_PIXEL, rule).unwrap();
    (0..counters.shape().rows())
        .find(|&row| counters.count(plane, row, 0).unwrap() > 0)
        .expect("попадание есть")
}

#[test]
fn gray_lands_on_the_same_row_as_on_the_rgb_waveform_for_every_code() {
    // Серый: Y' = код. Линии уровней и подписи у страниц общие — серый обязан лечь в ту же строку, что у RGB.
    let luma = LumaRule::new(4, 512).unwrap();
    let rgb = ParadeRule::new(4, 512).unwrap();
    for code in 0..=1023_u16 {
        assert_eq!(row_of(&luma, [code; 3], 0), row_of(&rgb, [code; 3], 0), "код {code}");
    }
}

#[test]
fn primaries_order_by_their_bt709_luma_weights() {
    let luma = LumaRule::new(4, 1024).unwrap();
    // Строка 0 — верх: светлее — меньше номер строки.
    let red = row_of(&luma, [1023, 0, 0], 0);
    let green = row_of(&luma, [0, 1023, 0], 0);
    let blue = row_of(&luma, [0, 0, 1023], 0);
    assert!(
        green < red && red < blue,
        "зелёный {green}, красный {red}, синий {blue}"
    );
    // Уровень красного — 0,2126 от полного размаха (1024 уровня на 1024 кода), с точностью до уровня.
    let red_level = 1023 - red;
    assert!(
        (f64::from(red_level) - 0.2126 * 1023.0).abs() <= 1.0,
        "уровень красного {red_level}"
    );
}

#[test]
fn one_plane_and_one_hit_per_sample_with_a_column_per_sample() {
    let luma = LumaRule::new(64, 128).unwrap();
    let gradient = picture(40, 6, |x, _| [(x * 25) as u16, 512, (1023 - x * 25) as u16]);
    let counters = accumulate(&gradient, SampleStride::EVERY_PIXEL, &luma).unwrap();
    assert_eq!(counters.shape().planes(), 1);
    assert_eq!(counters.shape().columns(), 40);
    assert_eq!(counters.total(), 40 * 6, "каждый отсчёт — ровно одно попадание");
}

#[test]
fn the_luma_page_resolves_to_one_white_section_with_the_waveform_level_lines() {
    let settings = ScopeModelSettings::default();
    let request = ScopeRequest {
        kind: ScopeKind::LumaWaveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: ScopeScan::RotatedClockwise,
        image_size: ScopeImageSize::try_new(300, 200).unwrap(),
    };
    let scope = ResolvedScope::resolve(&request, &settings, DisplayGamut::DisplayP3).unwrap();
    assert_eq!(scope.kind(), ScopeKind::LumaWaveform);
    assert_eq!(scope.trace_paint(), TracePaint::PlaneSections);
    assert_eq!(
        scope.scan(),
        ScopeScan::RotatedClockwise,
        "положение телефона читается, как у RGB"
    );
    // Пять линий уровней — те же, что у RGB waveform.
    let rgb = ResolvedScope::resolve(
        &ScopeRequest {
            kind: ScopeKind::Waveform,
            ..request
        },
        &settings,
        DisplayGamut::DisplayP3,
    )
    .unwrap();
    assert_eq!(scope.graticule(&settings.render), rgb.graticule(&settings.render));
}

#[test]
fn a_bright_flat_frame_paints_a_white_luma_trace_near_the_top_across_the_whole_width() {
    let settings = ScopeModelSettings {
        trace_max_columns: 64,
        trace_levels: 64,
        ..ScopeModelSettings::default()
    };
    let request = ScopeRequest {
        kind: ScopeKind::LumaWaveform,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan: ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(64, 64).unwrap(),
    };
    let scope = ResolvedScope::resolve(&request, &settings, DisplayGamut::Srgb).unwrap();
    // Светло-серый кадр: Y' ≈ 0,9 — след в верхней десятой части прибора.
    let shown = flat(64, 32, [920, 920, 920]);
    let counters = accumulate(&shown, SampleStride::EVERY_PIXEL, &scope).unwrap();
    let samples = SampleCount::for_window(64, 32, SampleStride::EVERY_PIXEL);
    let render_settings = RenderSettings {
        graticule_opacity: 0.0,
        ..settings.render.clone()
    };
    let picture = render(&counters, &scope, &render_settings, request.image_size, samples).unwrap();
    for x in [2, 32, 61] {
        let brightest_row =
            (0..64).max_by(|&a, &b| picture.pixel(x, a).unwrap()[3].total_cmp(&picture.pixel(x, b).unwrap()[3]));
        let row = brightest_row.unwrap();
        assert!(row <= 8, "столбец {x}: след в строке {row}");
        let pixel = picture.pixel(x, row).unwrap();
        assert_eq!(
            [pixel[0], pixel[1], pixel[2]],
            render_settings.luma_trace_color,
            "столбец {x}: белый след"
        );
        assert_eq!(picture.pixel(x, 50), Some([0.0; 4]), "столбец {x}: низ пуст");
    }
}
