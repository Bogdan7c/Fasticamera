//! Правило показа: счётчики → картинка RGBA с прямой альфой, по способу рисования прибора.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{flat, picture};
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, VectorscopeAxes};
use scope_model::{
    GraticulePrimitive, MAX_GRATICULE_PRIMITIVES, RenderError, RenderSettings, ResolvedScope, RgbWaveformPalette,
    SampleCount, SampleStride, ScopeCounters, ScopeModelSettings, ScopeRule, TracePaint, TraceShare, accumulate,
    hits_of_code, render,
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

/// Настройки, при которых у сетки из 10 столбцов порог полной яркости равен ровно 8 попаданиям:
/// `доля 1,0 · отсчётов / столбцов = 80 / 10`; отсчётов 80 даёт [`eighty_samples`].
fn threshold_eight() -> RenderSettings {
    RenderSettings {
        trace_full_share_of_column: TraceShare::try_new(1.0).unwrap(),
        ..without_graticule()
    }
}

/// 80 отсчётов (окно 80×1, каждый пиксель).
fn eighty_samples() -> SampleCount {
    SampleCount::for_window(80, 1, SampleStride::EVERY_PIXEL)
}

fn counters_with(scope: &ResolvedScope, cells: &[(u32, u32, u32, u32)]) -> ScopeCounters {
    let shape = scope.shape();
    let mut counts = vec![0; shape.cell_count()];
    for &(plane, row, column, count) in cells {
        counts[shape.index_of(plane, row, column).unwrap()] = count;
    }
    ScopeCounters::from_counts(shape, counts).unwrap()
}

fn assert_close(actual: [f32; 3], expected: [f32; 3], tolerance: f32, what: &str) {
    for channel in 0..3 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= tolerance,
            "{what}: {actual:?} против {expected:?}"
        );
    }
}

#[test]
fn empty_counters_without_graticule_render_fully_transparent() {
    for kind in ScopeKind::ALL {
        let scope = resolve(kind, VectorscopeAxes::YCbCrBt709);
        let counters = ScopeCounters::zeroed(scope.shape());
        let size = ScopeImageSize::try_new(20, 32).unwrap();
        let picture = render(&counters, &scope, &without_graticule(), size, eighty_samples()).unwrap();
        assert_eq!(picture.pixels().len(), 20 * 32);
        assert!(picture.pixels().iter().all(|&pixel| pixel == [0.0; 4]), "{kind:?}");
    }
}

#[test]
fn graticule_lines_appear_exactly_on_level_rows() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let counters = ScopeCounters::zeroed(scope.shape());
    let settings = RenderSettings::default();
    let size = ScopeImageSize::try_new(20, 160).unwrap();
    let picture = render(&counters, &scope, &settings, size, eighty_samples()).unwrap();
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
fn every_scope_has_a_non_empty_graticule_and_the_overlay_uses_the_same_five_levels_as_parade() {
    let settings = RenderSettings::default();
    for kind in ScopeKind::ALL {
        for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
            assert!(
                !resolve(kind, axes).graticule(&settings).is_empty(),
                "у {kind:?} {axes:?} нет сетки"
            );
        }
    }
    let waveform = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709).graticule(&settings);
    let parade = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709).graticule(&settings);
    assert_eq!(waveform.len(), 5);
    assert_eq!(waveform, parade);
}

#[test]
fn contract_level_lines_match_the_graticule_segments_one_to_one_within_a_row() {
    // Экран подписывает линии по списку словаря (решение 274), сетку кладёт правило попадания: подпись обязана стоять
    // у своей линии, а не у соседней и не зеркально (у waveform белое вверху).
    let settings = RenderSettings::default();
    for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
        let scope = resolve(kind, VectorscopeAxes::YCbCrBt709);
        let rows = f32::from(u16::try_from(small_settings().trace_levels).unwrap());
        let segment_ys: Vec<f32> = scope
            .graticule(&settings)
            .iter()
            .map(|primitive| match primitive {
                GraticulePrimitive::Segment { from, .. } => from[1],
                other => panic!("в сетке уровней не отрезок: {other:?}"),
            })
            .collect();
        let lines = kind.level_lines();
        assert_eq!(segment_ys.len(), lines.len(), "{kind:?}: линий и подписей поровну");
        for (line, segment_y) in lines.iter().zip(&segment_ys) {
            let distance_rows = (line.nominal_fraction_from_top() - segment_y).abs() * rows;
            assert!(
                distance_rows <= 0.5 + 1e-3,
                "{kind:?}: подпись {} стоит в {} строках от своей линии",
                line.label,
                distance_rows
            );
        }
    }
    assert!(ScopeKind::Vectorscope.level_lines().is_empty());
}

#[test]
fn every_scope_names_one_trace_paint_and_its_style_agrees() {
    let settings = RenderSettings::default();
    for (kind, axes, expected) in [
        (
            ScopeKind::Waveform,
            VectorscopeAxes::YCbCrBt709,
            TracePaint::PlanesOverlaid,
        ),
        (
            ScopeKind::RgbParade,
            VectorscopeAxes::YCbCrBt709,
            TracePaint::PlaneSections,
        ),
        (
            ScopeKind::Vectorscope,
            VectorscopeAxes::YCbCrBt709,
            TracePaint::ColorByPosition,
        ),
        (
            ScopeKind::Vectorscope,
            VectorscopeAxes::OklabAb,
            TracePaint::ColorByPosition,
        ),
    ] {
        let scope = resolve(kind, axes);
        assert_eq!(scope.trace_paint(), expected, "{kind:?} {axes:?}");
        assert_eq!(scope.trace_style(&settings).paint(), expected, "{kind:?} {axes:?}");
        // Способ требует столько плоскостей, сколько у прибора (кроме секций: им любое число).
        if let Some(required) = expected.required_planes() {
            assert_eq!(scope.shape().planes(), required, "{kind:?} {axes:?}");
        }
    }
}

#[test]
fn overlaid_trace_mixes_plane_primaries_and_alpha_follows_the_brightest_plane() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let settings = threshold_eight();
    // Строка 8: в столбце 0 только красная плоскость на пороге (8); в столбце 5 — красная и зелёная по 3.
    let counters = counters_with(&scope, &[(0, 8, 0, 8), (0, 8, 5, 3), (1, 8, 5, 3)]);
    let size = ScopeImageSize::try_new(10, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size, eighty_samples()).unwrap();
    let primaries = RgbWaveformPalette::Softened.primaries();

    // Одна плоскость на пороге: цвет — её основной цвет, делённый на свой наибольший канал; альфа — полная.
    let red_only = picture.pixel(0, 8).unwrap();
    let peak = primaries[0].iter().copied().fold(0.0, f32::max);
    assert_close(
        [red_only[0], red_only[1], red_only[2]],
        primaries[0].map(|channel| channel / peak),
        1e-6,
        "красная плоскость",
    );
    assert_eq!(red_only[3], settings.trace_opacity);

    // Две плоскости с одним счётчиком: смесь двух основных, альфа — по яркости ОДНОЙ плоскости, а не суммы.
    let mixed = picture.pixel(5, 8).unwrap();
    let intensity = 4.0_f32.log2() / 9.0_f32.log2();
    assert!(
        (mixed[3] - intensity * settings.trace_opacity).abs() < 1e-6,
        "альфа {}",
        mixed[3]
    );
    let sum = [0, 1, 2].map(|channel| primaries[0][channel] + primaries[1][channel]);
    let sum_peak = sum.iter().copied().fold(0.0, f32::max);
    assert_close(
        [mixed[0], mixed[1], mixed[2]],
        sum.map(|channel| channel / sum_peak),
        1e-6,
        "красная + зелёная",
    );
    // Пустое место рядом — прозрачно.
    assert_eq!(picture.pixel(4, 8), Some([0.0; 4]));
}

#[test]
fn gray_counters_overlay_to_a_white_trace_and_a_pure_palette_gives_pure_primaries() {
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let gray_cells = [(0, 8, 0, 8), (1, 8, 0, 8), (2, 8, 0, 8)];
    let counters = counters_with(&scope, &gray_cells);
    let size = ScopeImageSize::try_new(10, 16).unwrap();
    let picture = render(&counters, &scope, &threshold_eight(), size, eighty_samples()).unwrap();
    let gray = picture.pixel(0, 8).unwrap();
    assert_close([gray[0], gray[1], gray[2]], [1.0; 3], 1e-6, "серый кадр — белый след");
    assert_eq!(gray[3], threshold_eight().trace_opacity);

    // «Чистая» палитра: одна красная плоскость — чистый красный.
    let pure = RenderSettings {
        rgb_waveform_palette: RgbWaveformPalette::Pure,
        ..threshold_eight()
    };
    let red_only = counters_with(&scope, &[(0, 8, 0, 8)]);
    let picture = render(&red_only, &scope, &pure, size, eighty_samples()).unwrap();
    let red = picture.pixel(0, 8).unwrap();
    assert_close([red[0], red[1], red[2]], [1.0, 0.0, 0.0], 1e-6, "чистый красный");
    // Серый при «чистой» палитре — тоже белый (сумма основных единичная).
    let picture = render(&counters, &scope, &pure, size, eighty_samples()).unwrap();
    let gray = picture.pixel(0, 8).unwrap();
    assert_close([gray[0], gray[1], gray[2]], [1.0; 3], 1e-6, "серый, чистая палитра");
}

#[test]
fn overlay_spans_the_whole_width_while_parade_splits_it_into_sections() {
    let waveform = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let parade = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let cells = [(0, 4, 9, 8)];
    let size = ScopeImageSize::try_new(30, 16).unwrap();
    let settings = threshold_eight();
    // Наложение: столбец 9 из 10 — правая кромка всей картинки (пиксели 27..29), левее ничего нет.
    let overlay = render(
        &counters_with(&waveform, &cells),
        &waveform,
        &settings,
        size,
        eighty_samples(),
    )
    .unwrap();
    assert_eq!(overlay.pixel(10, 4), Some([0.0; 4]));
    assert!(overlay.pixel(28, 4).unwrap()[3] > 0.0);
    // Секции: та же ячейка красной плоскости видна только в первой трети (пиксели 9..11), правой кромки там нет.
    let sections = render(
        &counters_with(&parade, &cells),
        &parade,
        &settings,
        size,
        eighty_samples(),
    )
    .unwrap();
    assert_eq!(sections.pixel(28, 4), Some([0.0; 4]));
    assert!(sections.pixel(9, 4).unwrap()[3] > 0.0);
}

#[test]
fn saturated_cell_gets_full_trace_alpha_and_partial_cell_a_log_scaled_one_on_parade_sections() {
    let scope = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let settings = threshold_eight();
    // Ячейка (плоскость 0, строка 8, столбец 0) насыщена, (строка 8, столбец 5) — частично.
    let counters = counters_with(&scope, &[(0, 8, 0, 24), (0, 8, 5, 3)]);
    let size = ScopeImageSize::try_new(30, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size, eighty_samples()).unwrap();
    let saturated = picture.pixel(0, 8).unwrap();
    assert_eq!(saturated[3], settings.trace_opacity);
    assert_eq!([saturated[0], saturated[1], saturated[2]], settings.parade_colors[0]);
    // Столбец 5 из 10 в первой секции шириной 10 пикселей — пиксель 5.
    let partial = picture.pixel(5, 8).unwrap();
    let expected = (4.0_f32.log2() / 9.0_f32.log2()) * settings.trace_opacity;
    assert!((partial[3] - expected).abs() < 1e-6, "{} против {expected}", partial[3]);
    assert!(partial[3] < saturated[3]);
    assert_eq!(picture.pixel(4, 8), Some([0.0; 4]));
}

#[test]
fn parade_sections_take_their_plane_colour_and_cover_the_full_column_range() {
    let scope = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let settings = threshold_eight();
    // В каждой плоскости заполнен только столбец 0 строки 4, на пороге.
    let counters = counters_with(&scope, &[(0, 4, 0, 8), (1, 4, 0, 8), (2, 4, 0, 8)]);
    let size = ScopeImageSize::try_new(30, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size, eighty_samples()).unwrap();
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
fn the_same_counters_get_dimmer_when_the_frame_has_more_samples() {
    // Решение 266: полная яркость — с ДОЛИ отсчётов. Те же счётчики в кадре вдвое больше — слабее.
    let scope = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let settings = threshold_eight();
    let counters = counters_with(&scope, &[(0, 8, 0, 8)]);
    let size = ScopeImageSize::try_new(30, 16).unwrap();
    let alpha_with = |samples: SampleCount| {
        render(&counters, &scope, &settings, size, samples)
            .unwrap()
            .pixel(0, 8)
            .unwrap()[3]
    };
    let small = alpha_with(eighty_samples());
    let large = alpha_with(SampleCount::for_window(160, 1, SampleStride::EVERY_PIXEL));
    assert_eq!(
        small, settings.trace_opacity,
        "на пороге (8 при 80 отсчётах) — полная яркость"
    );
    assert!(large < small, "{large} против {small}");
    let expected = (9.0_f32.log2() / 17.0_f32.log2()) * settings.trace_opacity;
    assert!((large - expected).abs() < 1e-6, "{large} против {expected}");
}

#[test]
fn trace_is_composited_over_the_graticule_with_straight_alpha() {
    let scope = resolve(ScopeKind::RgbParade, VectorscopeAxes::YCbCrBt709);
    let settings = RenderSettings {
        trace_full_share_of_column: TraceShare::try_new(1.0).unwrap(),
        ..RenderSettings::default()
    };
    // Верхняя строка (100 %) совпадает с линией сетки и с насыщенным следом.
    let counters = counters_with(&scope, &[(0, 0, 0, 8)]);
    let size = ScopeImageSize::try_new(30, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size, eighty_samples()).unwrap();
    let pixel = picture.pixel(0, 0).unwrap();
    // Центр строки сетки 0 — y = 0,5: пиксель 0 покрыт линией не на 100 %, поэтому берём расчётное покрытие.
    let line = GraticulePrimitive::Segment {
        from: [0.0, 0.5 / 16.0],
        to: [1.0, 0.5 / 16.0],
        half_width_px: settings.graticule_half_width_px,
    };
    let grid_alpha = line.coverage([0.5, 0.5], [30.0, 16.0]) * settings.graticule_opacity;
    assert!(grid_alpha > 0.0);
    let expected_alpha = settings.trace_opacity + grid_alpha * (1.0 - settings.trace_opacity);
    assert!((pixel[3] - expected_alpha).abs() < 1e-6);
    // Цвет — смесь следа и сетки: красного в нём больше, чем в сетке, и меньше, чем в следе.
    assert!(pixel[0] < settings.parade_colors[0][0] + 1e-6 && pixel[0] > settings.graticule_color[0] - 1e-6);
    assert!(pixel[1] > settings.parade_colors[0][1], "зелёного от сетки прибавилось");
}

#[test]
fn counters_of_another_scope_are_rejected_and_bad_settings_are_reported() {
    let waveform = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let vectorscope = resolve(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709);
    let counters = ScopeCounters::zeroed(vectorscope.shape());
    let size = ScopeImageSize::try_new(8, 8).unwrap();
    assert_eq!(
        render(&counters, &waveform, &RenderSettings::default(), size, eighty_samples()),
        Err(RenderError::ShapeMismatch)
    );
    let bad_settings = RenderSettings {
        vectorscope_lightness: 2.0,
        ..RenderSettings::default()
    };
    assert!(matches!(
        render(&counters, &vectorscope, &bad_settings, size, eighty_samples()),
        Err(RenderError::Settings(_))
    ));
}

#[test]
fn white_picture_reaches_the_top_of_the_rendered_waveform_as_a_white_trace() {
    // Сквозной путь: картинка → счётчики → картинка прибора.
    let scope = resolve(ScopeKind::Waveform, VectorscopeAxes::YCbCrBt709);
    let shown = flat(40, 20, [1023; 3]);
    let counters = accumulate(&shown, SampleStride::EVERY_PIXEL, &scope).unwrap();
    let samples = SampleCount::for_window(40, 20, SampleStride::EVERY_PIXEL);
    let settings = without_graticule();
    let size = ScopeImageSize::try_new(10, 16).unwrap();
    let picture = render(&counters, &scope, &settings, size, samples).unwrap();
    for x in 0..10 {
        let top = picture.pixel(x, 0).unwrap();
        assert_eq!(top[3], settings.trace_opacity, "столбец {x}");
        assert_close([top[0], top[1], top[2]], [1.0; 3], 1e-6, "белый кадр — белый след");
        assert_eq!(picture.pixel(x, 8), Some([0.0; 4]));
        assert_eq!(picture.pixel(x, 15), Some([0.0; 4]));
    }
}

#[test]
fn counters_sum_to_samples_times_planes_for_waveform_but_never_exceed_samples_on_a_vectorscope() {
    // Знаменатель яркости берётся из числа отсчётов, а не из суммы счётчиков, именно потому, что вектороскоп
    // отбрасывает отсчёты вне сетки: сумма счётчиков там зависит от картинки.
    let (width, height) = (40, 20);
    let stride = SampleStride::EVERY_SECOND_PIXEL;
    let samples = SampleCount::for_window(width, height, stride);
    let any_picture = picture(width, height, |x, y| {
        [(x * 25) as u16, (y * 50) as u16, ((x + y) * 17) as u16]
    });
    for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
        let scope = resolve(kind, VectorscopeAxes::YCbCrBt709);
        let counters = accumulate(&any_picture, stride, &scope).unwrap();
        assert_eq!(
            counters.total(),
            samples.total() * 3,
            "{kind:?}: каждый отсчёт — по попаданию на плоскость"
        );
    }
    // Вектороскоп Y'CbCr: насыщенный красный при усилении 8 вылетает за сетку — сумма строго меньше отсчётов.
    let red = flat(width, height, [1023, 0, 0]);
    let amplified = ScopeModelSettings {
        ycbcr_gain: 8.0,
        ..small_settings()
    };
    let scope = ResolvedScope::resolve(
        &request(ScopeKind::Vectorscope, VectorscopeAxes::YCbCrBt709),
        &amplified,
        DisplayGamut::Srgb,
    )
    .unwrap();
    let counters = accumulate(&red, stride, &scope).unwrap();
    assert!(
        counters.total() < samples.total(),
        "{} против {}",
        counters.total(),
        samples.total()
    );
    // Серая картинка попадает в сетку целиком: сумма равна отсчётам.
    let gray = accumulate(&flat(width, height, [500; 3]), stride, &scope).unwrap();
    assert_eq!(gray.total(), samples.total());
}

#[test]
fn vectorscope_gray_picture_draws_one_bright_neutral_point_at_the_centre() {
    for axes in [VectorscopeAxes::YCbCrBt709, VectorscopeAxes::OklabAb] {
        let scope = resolve(ScopeKind::Vectorscope, axes);
        let shown = picture(16, 16, |x, y| [(x * 60 + y) as u16; 3]);
        let counters = accumulate(&shown, SampleStride::EVERY_PIXEL, &scope).unwrap();
        let samples = SampleCount::for_window(16, 16, SampleStride::EVERY_PIXEL);
        let settings = without_graticule();
        let size = ScopeImageSize::try_new(256, 256).unwrap();
        let picture = render(&counters, &scope, &settings, size, samples).unwrap();
        let lit: Vec<usize> = picture
            .pixels()
            .iter()
            .enumerate()
            .filter(|(_, pixel)| pixel[3] > 0.0)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(lit, vec![128 * 256 + 128], "{axes:?}");
        let centre = picture.pixel(128, 128).unwrap();
        assert_eq!(centre[3], settings.trace_opacity);
        // Серое — нейтраль светлоты вектороскопа: конечный цвет без NaN, все три канала равны.
        assert!(centre.iter().all(|channel| channel.is_finite()), "{axes:?}: {centre:?}");
        assert!(
            (centre[0] - centre[1]).abs() < 1e-5 && (centre[1] - centre[2]).abs() < 1e-5,
            "{axes:?}: {centre:?}"
        );
        if axes == VectorscopeAxes::YCbCrBt709 {
            assert_close(
                [centre[0], centre[1], centre[2]],
                [0.70; 3],
                1e-6,
                "нейтраль Y'CbCr = светлота",
            );
        }
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
