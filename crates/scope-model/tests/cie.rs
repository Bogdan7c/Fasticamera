//! Диаграмма CIE (сессия 14s): счётчики, окно, порог тёмного, независимость от поворота, сетка, подписи, картинка.
//!
//! Идут через настоящий путь продукта: запрос → решённый прибор → план отсчётов → накопление → картинка.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{flat, picture};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::image::DisplayGamut;
use scope_contract::{
    ChromaticityGamut, CieDiagram, ScopeImageSize, ScopeKind, ScopeMark, ScopeRequest, ScopeScan, VectorscopeAxes,
};
use scope_model::{
    ChartProjection, CieLocusFill, CieMark, CieRenderSettings, CieSettings, CieTraceColour, GraticulePrimitive,
    MAX_GRATICULE_PRIMITIVES, MAX_PLANCKIAN_SAMPLES, RenderSettings, ResolvedScope, ScopeModelSettings, ShownCodes,
    TracePaint, accumulate, render,
};

fn request(diagram: CieDiagram, scan: ScopeScan, size: u32) -> ScopeRequest {
    ScopeRequest {
        kind: ScopeKind::Cie,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: diagram,
        histogram_mode: scope_contract::HistogramMode::default(),
        scan,
        image_size: ScopeImageSize::try_new(size, size).unwrap(),
    }
}

fn resolve(diagram: CieDiagram, gamut: DisplayGamut, settings: &ScopeModelSettings) -> ResolvedScope {
    ResolvedScope::resolve(&request(diagram, ScopeScan::Upright, 512), settings, gamut).unwrap()
}

fn cie_rule(scope: &ResolvedScope) -> &scope_model::CieRule {
    match scope {
        ResolvedScope::Cie(rule) => rule,
        other => panic!("ожидалась диаграмма CIE: {other:?}"),
    }
}

/// Ячейка цветности `chromaticity` по формуле в f64 (то, что должно получиться без округлений f32).
fn exact_cell(rule: &scope_model::CieRule, chromaticity: Chromaticity) -> [f64; 2] {
    let projection = ChartProjection::new(rule.diagram(), rule.window());
    let point = projection.picture_point(chromaticity);
    let size = f64::from(rule.geometry().size());
    [f64::from(point[0]) * size, f64::from(point[1]) * size]
}

#[test]
fn the_cie_page_is_one_plane_square_grid_of_the_configured_size() {
    let settings = ScopeModelSettings::default();
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    assert_eq!(scope.kind(), ScopeKind::Cie);
    assert_eq!(scope.trace_paint(), TracePaint::Cie);
    let samples = scope.planned_samples(300, 200, &settings);
    let shape = scope_model::ScopeRule::shape(&scope, samples);
    assert_eq!((shape.columns(), shape.rows(), shape.planes()), (512, 512, 1));
}

#[test]
fn gray_lands_on_the_d65_cell_and_primaries_on_the_triangle_vertices_in_both_diagrams_and_gamuts() {
    let settings = ScopeModelSettings::default();
    for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let scope = resolve(diagram, gamut, &settings);
            let rule = cie_rule(&scope);
            let primaries = gamut.primaries();
            for (code, chromaticity, name) in [
                ([700, 700, 700], primaries.white, "белый"),
                ([1023, 0, 0], primaries.red, "красный"),
                ([0, 1023, 0], primaries.green, "зелёный"),
                ([0, 0, 1023], primaries.blue, "синий"),
            ] {
                let [column, row] = rule
                    .cell_of_code(code)
                    .unwrap_or_else(|| panic!("{diagram:?} {gamut:?} {name}: нет попадания"));
                let expected = exact_cell(rule, chromaticity);
                assert!(
                    (f64::from(column) + 0.5 - expected[0]).abs() <= 1.0
                        && (f64::from(row) + 0.5 - expected[1]).abs() <= 1.0,
                    "{diagram:?} {gamut:?} {name}: ячейка ({column}, {row}), ожидалось около {expected:?}"
                );
            }
        }
    }
}

#[test]
fn the_measured_gamut_decides_where_the_same_codes_lie() {
    // Один и тот же код красного: в sRGB-охвате это красный sRGB, в P3 — более насыщенный красный P3.
    let settings = ScopeModelSettings::default();
    let srgb = resolve(CieDiagram::Xy1931, DisplayGamut::Srgb, &settings);
    let p3 = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let code = [1023, 0, 0];
    let srgb_cell = cie_rule(&srgb).cell_of_code(code).unwrap();
    let p3_cell = cie_rule(&p3).cell_of_code(code).unwrap();
    assert!(
        p3_cell[0] > srgb_cell[0],
        "красный P3 правее красного sRGB: {p3_cell:?} против {srgb_cell:?}"
    );
}

#[test]
fn the_cloud_stays_inside_the_measured_gamut_triangle_for_random_codes() {
    // Решение 189: коды — картинка охвата измерения, облако внутри его треугольника (на ребре — клиппинг).
    let settings = ScopeModelSettings::default();
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let rule = cie_rule(&scope);
    let triangle = [
        DisplayGamut::DisplayP3.primaries().red,
        DisplayGamut::DisplayP3.primaries().green,
        DisplayGamut::DisplayP3.primaries().blue,
    ]
    .map(|vertex| exact_cell(rule, vertex));
    // Расстояние до стороны со знаком, в клетках (делим на длину стороны).
    let side = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / (b[0] - a[0]).hypot(b[1] - a[1])
    };
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 32) as u16 % 1024
    };
    let mut checked = 0;
    for _ in 0..5000 {
        let code = [next(), next(), next()];
        let Some([column, row]) = rule.cell_of_code(code) else {
            continue;
        };
        let point = [f64::from(column) + 0.5, f64::from(row) + 0.5];
        let signs: Vec<f64> = (0..3)
            .map(|i| side(triangle[i], triangle[(i + 1) % 3], point))
            .collect();
        // Допуск — две клетки на ребре (ячейка округляет, матрица в f32).
        let tolerance = 2.0;
        assert!(
            signs.iter().all(|value| *value >= -tolerance) || signs.iter().all(|value| *value <= tolerance),
            "код {code:?} вне треугольника: {signs:?}"
        );
        checked += 1;
    }
    assert!(checked > 4000);
}

#[test]
fn black_and_dark_codes_make_no_hit_and_the_threshold_is_a_setting() {
    let settings = ScopeModelSettings::default();
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let rule = cie_rule(&scope);
    assert_eq!(rule.cell_of_code([0, 0, 0]), None, "чёрный: знаменатель 0");
    assert_eq!(rule.cell_of_code([2, 2, 2]), None, "ниже порога яркости по умолчанию");
    // Порог по умолчанию — 1 % линейной яркости (решение 338): код 60 (≈ 0,45 %) отсекается, код 150 (≈ 1,9 %) — нет.
    assert_eq!(
        rule.cell_of_code([60, 60, 60]),
        None,
        "код 60 ниже 1 % линейной яркости"
    );
    assert!(rule.cell_of_code([150, 150, 150]).is_some());
    let permissive = ScopeModelSettings {
        cie: CieSettings {
            min_luminance: 0.0,
            ..CieSettings::default()
        },
        ..ScopeModelSettings::default()
    };
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &permissive);
    assert!(
        cie_rule(&scope).cell_of_code([2, 2, 2]).is_some(),
        "порог 0: тёмный код считается"
    );
    assert_eq!(
        cie_rule(&scope).cell_of_code([0, 0, 0]),
        None,
        "но чёрный всё равно без знаменателя"
    );
}

#[test]
fn counters_do_not_depend_on_how_the_eye_reads_the_picture() {
    let settings = ScopeModelSettings::default();
    let shown = picture(120, 90, |x, y| {
        [
            (x * 8) as u16 % 1024,
            (y * 11) as u16 % 1024,
            ((x + y) * 5) as u16 % 1024,
        ]
    });
    for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
        let mut reference = None;
        for scan in [
            ScopeScan::Upright,
            ScopeScan::RotatedClockwise,
            ScopeScan::UpsideDown,
            ScopeScan::RotatedCounterClockwise,
        ] {
            let scope =
                ResolvedScope::resolve(&request(diagram, scan, 512), &settings, DisplayGamut::DisplayP3).unwrap();
            let samples = scope.planned_samples(shown.width(), shown.height(), &settings);
            let counters = accumulate(&shown, samples.steps(), &scope).unwrap();
            assert!(counters.total() > 0);
            match &reference {
                None => reference = Some(counters),
                Some(first) => assert_eq!(&counters, first, "{diagram:?} {scan:?}"),
            }
        }
    }
}

#[test]
fn the_two_diagrams_put_the_same_colour_in_different_cells_by_the_published_conversion() {
    let settings = ScopeModelSettings::default();
    let xy = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let uv = resolve(CieDiagram::UvPrime1976, DisplayGamut::DisplayP3, &settings);
    let white_xy = cie_rule(&xy).cell_of_code([800, 800, 800]).unwrap();
    let white_uv = cie_rule(&uv).cell_of_code([800, 800, 800]).unwrap();
    assert_ne!(white_xy, white_uv);
    // D65 в u′v′ (0,1978; 0,4683) при окне (-0,03; -0,03; 0,68) и сетке 512.
    let expected_column = ((0.1978 + 0.03) / 0.68 * 512.0_f64) as i64;
    let expected_row = ((0.65 - 0.4683) / 0.68 * 512.0_f64) as i64;
    assert!((i64::from(white_uv[0]) - expected_column).abs() <= 1, "{white_uv:?}");
    assert!((i64::from(white_uv[1]) - expected_row).abs() <= 1, "{white_uv:?}");
}

fn primitives_of(scope: &ResolvedScope, render: &RenderSettings) -> Vec<GraticulePrimitive> {
    scope.graticule(render)
}

#[test]
fn the_default_graticule_is_the_locus_plus_the_p3_triangle_and_the_worst_case_fits_the_cap() {
    let settings = ScopeModelSettings::default();
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    // 64 отрезка локуса + линия пурпурных + 3 стороны P3.
    assert_eq!(primitives_of(&scope, &settings.render).len(), 65 + 3);
    let worst = RenderSettings {
        cie: CieRenderSettings {
            reference_gamuts: vec![
                ChromaticityGamut::Srgb,
                ChromaticityGamut::DisplayP3,
                ChromaticityGamut::Rec2020,
            ],
            marks: vec![CieMark::WhitePoint, CieMark::PlanckianLocus],
            planckian_samples: MAX_PLANCKIAN_SAMPLES,
            ..CieRenderSettings::default()
        },
        ..RenderSettings::default()
    };
    for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
        let scope = resolve(diagram, DisplayGamut::DisplayP3, &settings);
        let count = primitives_of(&scope, &worst).len();
        assert_eq!(count, 65 + 9 + 1 + (MAX_PLANCKIAN_SAMPLES as usize - 1));
        assert!(count <= MAX_GRATICULE_PRIMITIVES, "{count}");
    }
}

#[test]
fn the_p3_triangle_in_the_graticule_sits_where_the_rule_puts_the_p3_primary_codes() {
    // Сетка и след согласованы: вершина контура P3 стоит в той же ячейке, куда правило кладёт код основного цвета P3.
    let settings = ScopeModelSettings::default();
    for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
        let scope = resolve(diagram, DisplayGamut::DisplayP3, &settings);
        let rule = cie_rule(&scope);
        let segments: Vec<[f32; 2]> = primitives_of(&scope, &settings.render)
            .into_iter()
            .skip(65)
            .map(|primitive| match primitive {
                GraticulePrimitive::Segment { from, .. } => from,
                other => panic!("ожидался отрезок: {other:?}"),
            })
            .collect();
        for (vertex, code) in segments.iter().zip([[1023, 0, 0], [0, 1023, 0], [0, 0, 1023]]) {
            let [column, row] = rule.cell_of_code(code).unwrap();
            let size = 512.0;
            assert!(
                (vertex[0] * size - (column as f32 + 0.5)).abs() <= 1.5
                    && (vertex[1] * size - (row as f32 + 0.5)).abs() <= 1.5,
                "{diagram:?}: вершина {vertex:?}, ячейка ({column}, {row})"
            );
        }
    }
}

#[test]
fn the_white_mark_and_the_planckian_locus_are_drawn_only_when_enabled_and_the_white_circle_sits_on_gray() {
    let settings = ScopeModelSettings::default();
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let marked = RenderSettings {
        cie: CieRenderSettings {
            marks: vec![CieMark::WhitePoint],
            ..CieRenderSettings::default()
        },
        ..RenderSettings::default()
    };
    let primitives = primitives_of(&scope, &marked);
    assert_eq!(primitives.len(), 65 + 3 + 1);
    let Some(GraticulePrimitive::Ring { centre, .. }) = primitives.last().copied() else {
        panic!("последним должен быть кружок белой: {primitives:?}");
    };
    let [column, row] = cie_rule(&scope).cell_of_code([900, 900, 900]).unwrap();
    assert!(
        (centre[0] * 512.0 - (column as f32 + 0.5)).abs() <= 1.5
            && (centre[1] * 512.0 - (row as f32 + 0.5)).abs() <= 1.5
    );
}

#[test]
fn labels_follow_the_drawn_gamuts_stay_inside_the_picture_and_never_coincide() {
    let settings = ScopeModelSettings::default();
    let render = RenderSettings {
        cie: CieRenderSettings {
            reference_gamuts: vec![
                ChromaticityGamut::Srgb,
                ChromaticityGamut::DisplayP3,
                ChromaticityGamut::Rec2020,
            ],
            marks: vec![CieMark::WhitePoint],
            ..CieRenderSettings::default()
        },
        ..RenderSettings::default()
    };
    for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
        for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
            let labels = resolve(diagram, gamut, &settings).labels(&render);
            let marks: Vec<ScopeMark> = labels.iter().map(|label| label.mark).collect();
            assert_eq!(
                marks,
                [
                    ScopeMark::Gamut(ChromaticityGamut::Srgb),
                    ScopeMark::Gamut(ChromaticityGamut::DisplayP3),
                    ScopeMark::Gamut(ChromaticityGamut::Rec2020),
                    ScopeMark::WhitePoint
                ]
            );
            for (index, label) in labels.iter().enumerate() {
                assert!((0.0..=1.0).contains(&label.at.x()) && (0.0..=1.0).contains(&label.at.y()));
                for other in &labels[index + 1..] {
                    let distance = (label.at.x() - other.at.x()).hypot(label.at.y() - other.at.y());
                    assert!(
                        distance >= scope_model::MIN_LABEL_SEPARATION * 0.99,
                        "{diagram:?} {gamut:?}: {label:?} / {other:?}"
                    );
                }
            }
        }
    }
    let default_labels =
        resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings).labels(&RenderSettings::default());
    assert_eq!(default_labels.len(), 1);
    assert_eq!(default_labels[0].mark, ScopeMark::Gamut(ChromaticityGamut::DisplayP3));
}

fn shown_gray_and_green() -> ShownCodes {
    picture(64, 64, |x, _| if x < 32 { [700, 700, 700] } else { [100, 1000, 100] })
}

fn render_cie(settings: &ScopeModelSettings, diagram: CieDiagram) -> scope_model::ScopePicture {
    let shown = shown_gray_and_green();
    let scope = resolve(diagram, DisplayGamut::DisplayP3, settings);
    let samples = scope.planned_samples(64, 64, settings);
    let counters = accumulate(&shown, samples.steps(), &scope).unwrap();
    render(
        &counters,
        &scope,
        &settings.render,
        ScopeImageSize::try_new(512, 512).unwrap(),
        samples,
    )
    .unwrap()
}

#[test]
fn the_white_trace_is_white_where_the_gray_cloud_lies_and_the_chromaticity_trace_is_coloured_on_the_green() {
    let mut settings = ScopeModelSettings::default();
    settings.render.graticule_opacity = 0.0;
    // Белый след — явная настройка (умолчание с решения 338 — цвет по цветности).
    settings.render.cie.trace_colour = CieTraceColour::White;
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let [gray_column, gray_row] = cie_rule(&scope).cell_of_code([700, 700, 700]).unwrap();
    let [green_column, green_row] = cie_rule(&scope).cell_of_code([100, 1000, 100]).unwrap();
    let white_trace = render_cie(&settings, CieDiagram::Xy1931);
    let gray_pixel = white_trace.pixel(gray_column, gray_row).unwrap();
    assert!(gray_pixel[3] > 0.5, "серое облако ярко: {gray_pixel:?}");
    assert_eq!(&gray_pixel[..3], &[1.0, 1.0, 1.0]);
    assert_eq!(white_trace.pixel(10, 500).unwrap()[3], 0.0, "вдали от облака пусто");
    settings.render.cie.trace_colour = CieTraceColour::ByChromaticity;
    let coloured = render_cie(&settings, CieDiagram::Xy1931);
    let green = coloured.pixel(green_column, green_row).unwrap();
    assert!(
        green[3] > 0.5 && green[1] > green[0] && green[1] > green[2] && green[1] > 0.9,
        "{green:?}"
    );
    let gray = coloured.pixel(gray_column, gray_row).unwrap();
    assert!(
        gray[..3].iter().all(|channel| *channel > 0.97),
        "серый почти белый: {gray:?}"
    );
}

#[test]
fn the_faint_locus_fill_is_coloured_inside_the_locus_and_absent_outside_while_the_outline_option_has_none() {
    let mut settings = ScopeModelSettings::default();
    settings.render.graticule_opacity = 0.0;
    settings.render.cie.locus_fill = CieLocusFill::FaintColour;
    for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
        let picture = render_cie(&settings, diagram);
        // Левый верхний угол вне локуса, середина левее белого внутри; облако занимает малые пятна.
        assert_eq!(picture.pixel(3, 3).unwrap()[3], 0.0, "{diagram:?}: угол вне локуса");
        let inside = picture.pixel(256, 300).unwrap();
        assert!(
            (inside[3] - settings.render.cie.locus_fill_opacity).abs() < 1e-5,
            "{diagram:?}: {inside:?}"
        );
        // Заливка бледная и цветная: каналы разные (это цвет цветности, не серый).
        assert!(
            inside[0].max(inside[1]).max(inside[2]) - inside[0].min(inside[1]).min(inside[2]) > 0.05,
            "{inside:?}"
        );
    }
    settings.render.cie.locus_fill = CieLocusFill::Outline;
    assert_eq!(
        render_cie(&settings, CieDiagram::Xy1931).pixel(256, 300).unwrap()[3],
        0.0,
        "без заливки внутри локуса пусто"
    );
    let flat_frame = flat(8, 8, [0, 0, 0]);
    let scope = resolve(CieDiagram::Xy1931, DisplayGamut::DisplayP3, &settings);
    let samples = scope.planned_samples(8, 8, &settings);
    let counters = accumulate(&flat_frame, samples.steps(), &scope).unwrap();
    assert_eq!(counters.total(), 0, "чёрный кадр: ни одного попадания");
}
