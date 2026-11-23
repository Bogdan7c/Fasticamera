//! Правило показа: счётчики → картинка RGBA с прямой альфой.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{flat, picture};
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    GraticulePrimitive, MAX_GRATICULE_PRIMITIVES, RenderError, RenderSettings, ResolvedScope, SampleStride,
    ScopeCounters, ScopeModelSettings, ScopeRule, accumulate, hits_of_code, render, trace_intensity,
};

fn request(kind: ScopeKind, axes: VectorscopeAxes) -> ScopeRequest {
    ScopeRequest {
        kind,
        vectorscope_axes: axes,
        scan: scope_contract::ScopeScan::Upright,
        image_size: ScopeImageSize::try_new(64, 64).unwrap(),
    }
}

fn small_settings() -> ScopeModelSettings {
    ScopeModelSettings {
        trace_columns: 10,
        trace_levels: 16,
        ..ScopeModelSettings::default()
    }
}

fn resolve(kind: ScopeKind, axes: VectorscopeAxes) -> ResolvedScope {
    ResolvedScope::resolve(&request(kind, axes), &small_settings(), DisplayGamut::DisplayP3).unwrap()
}

fn without_graticule() -> RenderSettings {
    RenderSettings {
        graticule_opacity: 0.0,
        ..RenderSettings::default()
    }
}

fn counters_with(scope: &ResolvedScope, cells: &[(u32, u32, u32, u32)]) -> ScopeCounters {
    let shape = scope.shape();
    let mut counts = vec![0; shape.cell_count()];
    for &(plane, row, column, count) in cells {
        counts[shape.index_of(plane, row, column).unwrap()] = count;
    }
    ScopeCounters::from_counts(shape, counts).unwrap()
}

#[test]
fn empty_counters_without_graticule_render_fully_transparent() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let counters = ScopeCounters::zeroed(scope.shape());
    let size = ScopeImageSize::try_new(20, 32).unwrap();
    let picture = render(&counters, &scope, &without_graticule(), size).unwrap();
    assert_eq!(picture.pixels().len(), 20 * 32);
    assert!(picture.pixels().iter().all(|&pixel| pixel == [0.0; 4]));
}

#[test]
fn graticule_lines_appear_exactly_on_level_rows() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let counters = ScopeCounters::zeroed(scope.shape());
    let settings = RenderSettings::default();
    let size = ScopeImageSize::try_new(20, 160).unwrap();
    let picture = render(&counters, &scope, &settings, size).unwrap();
    // 16 уровней, высота 160: уровень 0 % → строка сетки 15 → линия на y = 155,0 (граница пикселей 154 и 155):
    // расстояние до центра пикселя 0,5, покрытие 0,75 + 0,5 − 0,5 = 0,75.
    let expected_alpha = 0.75 * settings.graticule_opacity;
    for y in [154, 155] {
        let on_line = picture.pixel(3, y).unwrap();
        assert!((on_line[3] - expected_alpha).abs() < 1e-6, "строка {y}: {}", on_line[3]);
        assert_eq!([on_line[0], on_line[1], on_line[2]], settings.graticule_color);
    }
    // 100 % → строка сетки 0 → y = 5,0.
    assert!((picture.pixel(3, 4).unwrap()[3] - expected_alpha).abs() < 1e-6);
    // Рядом с линией покрытия уже нет: расстояние 1,5 → 0,75 + 0,5 − 1,5 < 0.
    assert_eq!(picture.pixel(3, 153), Some([0.0; 4]));
    // Между линиями — полностью прозрачно.
    assert_eq!(picture.pixel(3, 95), Some([0.0; 4]));
}

#[test]
fn saturated_cell_gets_full_trace_alpha_and_partial_cell_a_log_scaled_one() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let settings = without_graticule();
    let reference = settings.trace_saturation_count;
    // Ячейка (строка 8, столбец 0) насыщена, (строка 8, столбец 5) — частично.
    let counters = counters_with(&scope, &[(0, 8, 0, reference * 3), (0, 8, 5, 3)]);
    let size = ScopeImageSize::try_new(10, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size).unwrap();
    let saturated = picture.pixel(0, 8).unwrap();
    assert_eq!(saturated[3], settings.trace_opacity);
    assert_eq!([saturated[0], saturated[1], saturated[2]], settings.waveform_color);
    let partial = picture.pixel(5, 8).unwrap();
    let expected = (4.0_f32.log2() / 9.0_f32.log2()) * settings.trace_opacity;
    assert!((partial[3] - expected).abs() < 1e-6, "{} против {expected}", partial[3]);
    assert!(partial[3] < saturated[3]);
    assert_eq!(picture.pixel(4, 8), Some([0.0; 4]));
    assert_eq!(trace_intensity(reference, reference), 1.0);
    assert_eq!(trace_intensity(0, reference), 0.0);
}

#[test]
fn parade_sections_take_their_plane_colour_and_cover_the_full_column_range() {
    let scope = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let settings = without_graticule();
    let reference = settings.trace_saturation_count;
    // В каждой плоскости заполнен только столбец 0 строки 4.
    let counters = counters_with(
        &scope,
        &[(0, 4, 0, reference), (1, 4, 0, reference), (2, 4, 0, reference)],
    );
    let size = ScopeImageSize::try_new(30, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size).unwrap();
    for (plane, section_start) in [(0usize, 0u32), (1, 10), (2, 20)] {
        let pixel = picture.pixel(section_start, 4).unwrap();
        assert_eq!(
            [pixel[0], pixel[1], pixel[2]],
            settings.parade_colors[plane],
            "секция {plane}"
        );
        assert_eq!(pixel[3], settings.trace_opacity);
        // Середина секции — столбец 5 сетки, он пуст.
        assert_eq!(picture.pixel(section_start + 5, 4), Some([0.0; 4]));
    }
}

#[test]
fn trace_is_composited_over_the_graticule_with_straight_alpha() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let settings = RenderSettings::default();
    // Верхняя строка (100 %) совпадает с линией сетки и с насыщенным следом.
    let counters = counters_with(&scope, &[(0, 0, 0, settings.trace_saturation_count)]);
    let size = ScopeImageSize::try_new(10, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size).unwrap();
    let pixel = picture.pixel(0, 0).unwrap();
    // Центр строки сетки 0 — y = 0,5: пиксель 0 покрыт линией не на 100 %, поэтому берём расчётное покрытие.
    let line = GraticulePrimitive::Segment {
        from: [0.0, 0.5 / 16.0],
        to: [1.0, 0.5 / 16.0],
        half_width_px: settings.graticule_half_width_px,
    };
    let grid_alpha = line.coverage([0.5, 0.5], [10.0, 16.0]) * settings.graticule_opacity;
    assert!(grid_alpha > 0.0);
    let expected_alpha = settings.trace_opacity + grid_alpha * (1.0 - settings.trace_opacity);
    assert!((pixel[3] - expected_alpha).abs() < 1e-6);
    // Цвет — смесь следа и сетки: зелёного в нём больше, чем в сетке, и меньше, чем в следе.
    assert!(pixel[1] < settings.waveform_color[1] + 1e-6 && pixel[1] > settings.graticule_color[1] - 1e-6);
}

#[test]
fn counters_of_another_scope_are_rejected() {
    let waveform = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let parade = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let counters = ScopeCounters::zeroed(parade.shape());
    let size = ScopeImageSize::try_new(8, 8).unwrap();
    assert_eq!(
        render(&counters, &waveform, &RenderSettings::default(), size),
        Err(RenderError::ShapeMismatch)
    );
    let bad_settings = RenderSettings {
        trace_saturation_count: 0,
        ..RenderSettings::default()
    };
    assert!(matches!(
        render(&counters, &parade, &bad_settings, size),
        Err(RenderError::Settings(_))
    ));
}

#[test]
fn white_picture_reaches_the_top_of_the_rendered_waveform() {
    // Сквозной путь: картинка → счётчики → картинка прибора.
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let shown = flat(40, 20, [1023; 3]);
    let counters = accumulate(&shown, SampleStride::EVERY_PIXEL, &scope).unwrap();
    let settings = without_graticule();
    let size = ScopeImageSize::try_new(10, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size).unwrap();
    for x in 0..10 {
        assert_eq!(picture.pixel(x, 0).unwrap()[3], settings.trace_opacity, "столбец {x}");
        assert_eq!(picture.pixel(x, 8), Some([0.0; 4]));
        assert_eq!(picture.pixel(x, 15), Some([0.0; 4]));
    }
}

#[test]
fn vectorscope_gray_picture_draws_one_bright_point_at_the_centre() {
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let scope = resolve(ScopeKind::Vectorscope, axes);
        let shown = picture(16, 16, |x, y| [(x * 60 + y) as u16; 3]);
        let counters = accumulate(&shown, SampleStride::EVERY_PIXEL, &scope).unwrap();
        let settings = without_graticule();
        let size = ScopeImageSize::try_new(256, 256).unwrap();
        let picture = render(&counters, &scope, &settings, size).unwrap();
        let lit: Vec<usize> = picture
            .pixels()
            .iter()
            .enumerate()
            .filter(|(_, pixel)| pixel[3] > 0.0)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(lit, vec![128 * 256 + 128], "{axes:?}");
        assert_eq!(picture.pixel(128, 128).unwrap()[3], settings.trace_opacity);
    }
}

#[test]
fn graticule_primitive_count_is_bounded_for_every_scope() {
    let settings = RenderSettings::default();
    for kind in ScopeKind::ALL {
        for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
            let primitives = resolve(kind, axes).graticule(&settings);
            assert!(!primitives.is_empty(), "{kind:?} {axes:?}");
            assert!(primitives.len() <= MAX_GRATICULE_PRIMITIVES);
        }
    }
}

#[test]
fn vectorscope_targets_sit_where_the_hit_rule_puts_the_bar_colours() {
    let settings = RenderSettings::default();
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let scope = resolve(ScopeKind::Vectorscope, axes);
        let boxes: Vec<[f32; 2]> = scope
            .graticule(&settings)
            .into_iter()
            .filter_map(|primitive| match primitive {
                GraticulePrimitive::BoxOutline { centre, .. } => Some(centre),
                _ => None,
            })
            .collect();
        assert_eq!(boxes.len(), 6, "{axes:?}");
        let on = (0.75_f32 * 1023.0).round() as u16;
        let red_hit = hits_of_code(
            &scope,
            scope_model::SamplePosition { x: 0, picture_width: 1 },
            [on, 0, 0],
        )[0];
        let expected = [
            (red_hit.column as f32 + 0.5) / 256.0,
            (red_hit.row as f32 + 0.5) / 256.0,
        ];
        assert_eq!(boxes[0], expected, "{axes:?}");
    }
}

#[test]
fn skin_line_follows_the_configured_angle_on_ycbcr_and_points_orange_on_oklab() {
    let settings = RenderSettings::default();
    let skin_direction = |axes| {
        let scope = resolve(ScopeKind::Vectorscope, axes);
        let segments: Vec<([f32; 2], [f32; 2])> = scope
            .graticule(&settings)
            .into_iter()
            .filter_map(|primitive| match primitive {
                GraticulePrimitive::Segment { from, to, .. } => Some((from, to)),
                _ => None,
            })
            .collect();
        // Крест — два первых отрезка, линия кожи — третий.
        let (from, to) = segments[2];
        f32::atan2(-(to[1] - from[1]), to[0] - from[0])
            .to_degrees()
            .rem_euclid(360.0)
    };
    assert!((skin_direction(VectorscopeAxes::YCbCrBt709) - 123.0).abs() < 0.01);
    // Оттенок кожи в OKLab — тёплый оранжевый, первая четверть (a > 0, b > 0), около 40–80°.
    let oklab = skin_direction(VectorscopeAxes::OklabAb);
    assert!((30.0..90.0).contains(&oklab), "оттенок кожи {oklab}°");
}
