//! Гистограмма (сессия 14s): счётчики по уровням, рисование столбиков, шкалы высоты, сетка.
//!
//! Тесты идут через настоящий путь продукта: запрос → решённый прибор → план отсчётов → накопление → картинка. Проверяется
//! результат (в какой уровень легли отсчёты, какого цвета и высоты столбик в картинке), а не вызов функций.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{flat, picture};
use isp_model::image::DisplayGamut;
use scope_contract::{HistogramMode, ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{
    GraticulePrimitive, HistogramHeight, RenderSettings, ResolvedScope, ScopeCounters, ScopeModelSettings,
    ScopePicture, ShownCodes, TracePaint, accumulate, render,
};

const BINS: u32 = 512;

fn request(mode: HistogramMode, scan: ScopeScan, size: ScopeImageSize) -> ScopeRequest {
    ScopeRequest {
        kind: ScopeKind::Histogram,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: mode,
        scan,
        image_size: size,
    }
}

fn size(width: u32, height: u32) -> ScopeImageSize {
    ScopeImageSize::try_new(width, height).unwrap()
}

/// Настройки ИЗМЕРЕНИЯ высоты: без сетки, заливка непрозрачная и полного цвета, линии и зазора нет, поэтому альфа пикселя
/// равна покрытию столбиком (как до решения 337), а сумма альф по столбцу — высоте столбика в пикселях.
fn settings_with_height(height: HistogramHeight) -> ScopeModelSettings {
    ScopeModelSettings {
        render: RenderSettings {
            graticule_opacity: 0.0,
            histogram_graticule_opacity: 0.0,
            histogram_height: height,
            histogram_fill_opacity: 1.0,
            histogram_fill_tint: 1.0,
            histogram_edge_width_px: 0.0,
            histogram_strip_gap: 0.0,
            ..RenderSettings::default()
        },
        ..ScopeModelSettings::default()
    }
}

/// Настройки ВИДА: умолчания продукта (заливка 0,28, линия 1,25 px, зазор) без сетки и с заданным числом `edit`.
fn look_settings(edit: impl FnOnce(&mut RenderSettings)) -> ScopeModelSettings {
    let mut render = RenderSettings {
        graticule_opacity: 0.0,
        histogram_graticule_opacity: 0.0,
        histogram_height: HistogramHeight::LinearToPeak,
        ..RenderSettings::default()
    };
    edit(&mut render);
    ScopeModelSettings {
        render,
        ..ScopeModelSettings::default()
    }
}

fn resolve(mode: HistogramMode, scan: ScopeScan, settings: &ScopeModelSettings) -> ResolvedScope {
    ResolvedScope::resolve(&request(mode, scan, size(64, 64)), settings, DisplayGamut::Srgb).unwrap()
}

/// Счётчики кадра по плану продукта (шаги прибора из настроек).
fn counters_of(shown: &ShownCodes, scope: &ResolvedScope, settings: &ScopeModelSettings) -> ScopeCounters {
    let samples = scope.planned_samples(shown.width(), shown.height(), settings);
    accumulate(shown, samples.steps(), scope).unwrap()
}

/// Картинка гистограммы кадра `shown` размера `picture_size`.
fn picture_of(
    shown: &ShownCodes,
    mode: HistogramMode,
    settings: &ScopeModelSettings,
    picture_size: ScopeImageSize,
) -> ScopePicture {
    let scope = ResolvedScope::resolve(
        &request(mode, ScopeScan::Upright, picture_size),
        settings,
        DisplayGamut::Srgb,
    )
    .unwrap();
    let samples = scope.planned_samples(shown.width(), shown.height(), settings);
    let counters = accumulate(shown, samples.steps(), &scope).unwrap();
    render(&counters, &scope, &settings.render, picture_size, samples).unwrap()
}

/// Сколько пикселей столбца `x` закрашено (сумма покрытий, то есть высота столбика в пикселях с мягким верхом).
fn bar_height_px(picture: &ScopePicture, x: u32) -> f32 {
    (0..picture.size().height())
        .map(|y| picture.pixel(x, y).unwrap()[3])
        .sum()
}

fn level_of_code(code: u16) -> u32 {
    u32::from(code) * BINS / 1024
}

#[test]
fn a_gray_frame_puts_every_sample_into_the_gray_level_of_every_plane_and_luma_agrees() {
    let settings = ScopeModelSettings::default();
    for code in [0_u16, 1, 255, 512, 700, 1022, 1023] {
        let shown = flat(300, 200, [code; 3]);
        for mode in [HistogramMode::Rgb, HistogramMode::Parade] {
            let scope = resolve(mode, ScopeScan::Upright, &settings);
            let counters = counters_of(&shown, &scope, &settings);
            let total = scope.planned_samples(300, 200, &settings).total() as u32;
            assert_eq!((counters.shape().columns(), counters.shape().rows()), (BINS, 1));
            for plane in 0..3 {
                for bin in 0..BINS {
                    let expected = if bin == level_of_code(code) { total } else { 0 };
                    assert_eq!(
                        counters.count(plane, 0, bin),
                        Some(expected),
                        "код {code}, плоскость {plane}, уровень {bin}"
                    );
                }
            }
        }
        let luma = resolve(HistogramMode::Luma, ScopeScan::Upright, &settings);
        let counters = counters_of(&shown, &luma, &settings);
        assert_eq!(counters.shape().planes(), 1);
        assert_eq!(
            counters.count(0, 0, level_of_code(code)),
            Some(luma.planned_samples(300, 200, &settings).total() as u32),
            "яркость серого {code} лежит в том же уровне"
        );
        assert_eq!(counters.total(), luma.planned_samples(300, 200, &settings).total());
    }
}

#[test]
fn a_saturated_red_frame_fills_the_top_level_of_red_and_the_bottom_level_of_green_and_blue() {
    let settings = ScopeModelSettings::default();
    let scope = resolve(HistogramMode::Rgb, ScopeScan::Upright, &settings);
    let counters = counters_of(&flat(120, 80, [1023, 0, 0]), &scope, &settings);
    let total = scope.planned_samples(120, 80, &settings).total() as u32;
    assert_eq!(counters.count(0, 0, BINS - 1), Some(total));
    assert_eq!(counters.count(1, 0, 0), Some(total));
    assert_eq!(counters.count(2, 0, 0), Some(total));
    assert_eq!(counters.total(), 3 * u64::from(total));
}

#[test]
fn counters_do_not_depend_on_the_phone_orientation() {
    // Асимметричная картинка с текстурой по обеим осям: любое чтение «слева направо» иначе дало бы другие счётчики.
    let settings = ScopeModelSettings::default();
    let shown = picture(257, 113, |x, y| {
        [
            (x * 4 % 1024) as u16,
            (y * 9 % 1024) as u16,
            ((x * 3 + y * 5) % 1024) as u16,
        ]
    });
    for mode in [HistogramMode::Rgb, HistogramMode::Parade, HistogramMode::Luma] {
        let reference = counters_of(&shown, &resolve(mode, ScopeScan::Upright, &settings), &settings);
        assert!(reference.total() > 0);
        for scan in [
            ScopeScan::RotatedClockwise,
            ScopeScan::UpsideDown,
            ScopeScan::RotatedCounterClockwise,
        ] {
            let rotated = counters_of(&shown, &resolve(mode, scan, &settings), &settings);
            assert_eq!(rotated, reference, "{mode:?} при положении {scan:?}");
        }
    }
}

#[test]
fn a_gray_frame_paints_one_white_bar_where_the_gray_level_is() {
    let settings = settings_with_height(HistogramHeight::LinearToPeak);
    let picture_size = size(1024, 100);
    let picture = picture_of(&flat(200, 100, [512; 3]), HistogramMode::Rgb, &settings, picture_size);
    // Уровень 256 из 512 на ширине 1024: центр столбца — x = 513.
    let bar_x = 512;
    let bottom = picture.pixel(bar_x, 99).unwrap();
    for channel in 0..3 {
        assert!(
            (bottom[channel] - 1.0).abs() < 1e-5,
            "три плоскости совпали — белый: {bottom:?}"
        );
    }
    assert!(
        (bottom[3] - 1.0).abs() < 1e-6,
        "низ столбика полностью закрашен: {bottom:?}"
    );
    assert!(
        bar_height_px(&picture, bar_x) > 99.0,
        "самый высокий столбик достаёт до верха"
    );
    // Далеко от столбика пусто на всей высоте, в том числе внизу.
    for x in [20, 200, 800, 1000] {
        assert_eq!(picture.pixel(x, 99), Some([0.0; 4]), "x {x}");
    }
}

#[test]
fn rgb_fills_take_the_primary_of_their_plane_and_add_up_where_planes_coincide() {
    let settings = settings_with_height(HistogramHeight::LinearToPeak);
    let picture_size = size(1024, 100);
    // Красный кадр: R — в верхнем уровне (правый край), G и B — в нижнем (левый край).
    let picture = picture_of(
        &flat(200, 100, [1023, 0, 0]),
        HistogramMode::Rgb,
        &settings,
        picture_size,
    );
    let primaries = settings.render.rgb_waveform_palette.primaries();
    let right = picture.pixel(1020, 99).unwrap();
    let left = picture.pixel(3, 99).unwrap();
    let green_plus_blue: [f32; 3] =
        std::array::from_fn(|channel| (primaries[1][channel] + primaries[2][channel]).min(1.0));
    for channel in 0..3 {
        assert!(
            (right[channel] - primaries[0][channel]).abs() < 1e-5,
            "правый столбик — красный: {right:?}"
        );
        assert!(
            (left[channel] - green_plus_blue[channel]).abs() < 1e-5,
            "левый столбик — зелёный и синий вместе (голубой): {left:?}"
        );
    }
    assert!(right[0] > right[1] && left[0] < left[1]);
}

#[test]
fn parade_stacks_three_strips_top_to_bottom_each_with_its_own_baseline() {
    let settings = settings_with_height(HistogramHeight::LinearToPeak);
    // Высота 300 без зазора: полосы по 100 px — R сверху, G, B снизу.
    let picture = picture_of(
        &flat(200, 100, [512; 3]),
        HistogramMode::Parade,
        &settings,
        size(1024, 300),
    );
    let colors = settings.render.parade_colors;
    for (strip, color) in colors.iter().enumerate() {
        let baseline_row = strip as u32 * 100 + 99;
        let pixel = picture.pixel(512, baseline_row).unwrap();
        for channel in 0..3 {
            assert!(
                (pixel[channel] - color[channel]).abs() < 1e-5,
                "полоса {strip}: {pixel:?}"
            );
        }
        assert!(
            (pixel[3] - 1.0).abs() < 1e-6,
            "у базовой линии полосы {strip} столбик закрашен"
        );
        // Столбик достаёт до верха СВОЕЙ полосы (серый кадр — пик у всех трёх), а выше — уже чужая полоса.
        let top_row = strip as u32 * 100;
        assert!(picture.pixel(512, top_row).unwrap()[3] > 0.9, "полоса {strip}: верх");
        // Каждая полоса занимает всю ширину: слева от столбика пусто на всей её высоте.
        for row in [top_row, top_row + 50, baseline_row] {
            assert_eq!(picture.pixel(20, row), Some([0.0; 4]), "полоса {strip}, строка {row}");
        }
    }
}

#[test]
fn a_pure_red_frame_lights_only_the_right_end_of_the_top_parade_strip() {
    let settings = settings_with_height(HistogramHeight::LinearToPeak);
    let picture = picture_of(
        &flat(200, 100, [1023, 0, 0]),
        HistogramMode::Parade,
        &settings,
        size(1024, 300),
    );
    let column_alpha =
        |x: u32, rows: std::ops::Range<u32>| -> f32 { rows.map(|y| picture.pixel(x, y).unwrap()[3]).sum() };
    // Красный — на правом конце ТОЛЬКО верхней полосы: заливка (0,28) почти на всей её высоте.
    assert!(column_alpha(1022, 0..100) > 20.0);
    assert_eq!(column_alpha(1022, 100..300), 0.0, "в G и B правого конца нет");
    // Зелёный и синий — на левом конце своих полос, красного там нет.
    assert_eq!(column_alpha(1, 0..100), 0.0);
    assert!(column_alpha(1, 100..200) > 20.0 && column_alpha(1, 200..300) > 20.0);
}

#[test]
fn parade_strips_are_separated_by_a_gap_in_which_nothing_is_drawn() {
    let settings = look_settings(|render| render.histogram_strip_gap = 0.05);
    let height = 300;
    let picture = picture_of(
        &flat(200, 100, [512; 3]),
        HistogramMode::Parade,
        &settings,
        size(1024, height),
    );
    let strips = scope_model::HistogramStrips::new(3, 0.05);
    let mut lit_rows = [0_u32; 3];
    for y in 0..height {
        let alpha = picture.pixel(512, y).unwrap()[3];
        match strips.locate(y as f32 + 0.5, height as f32) {
            Some(strip) => lit_rows[strip.index as usize] += u32::from(alpha > 0.0),
            None => assert_eq!(alpha, 0.0, "строка {y} лежит в зазоре"),
        }
    }
    // Высота полосы (300 − 2 · 15) / 3 = 90: столбик серого кадра закрывает её целиком.
    assert_eq!(lit_rows, [90; 3]);
}

#[test]
fn luma_draws_one_graph_in_the_luma_colour_over_the_whole_width_and_height() {
    let settings = settings_with_height(HistogramHeight::LinearToPeak);
    let picture_size = size(1024, 100);
    let picture = picture_of(&flat(200, 100, [768; 3]), HistogramMode::Luma, &settings, picture_size);
    // Уровень 384 из 512: центр столбца — x = 769.
    let pixel = picture.pixel(768, 99).unwrap();
    assert_eq!([pixel[0], pixel[1], pixel[2]], settings.render.luma_trace_color);
    assert!(bar_height_px(&picture, 768) > 99.0, "одна полоса на всю высоту");
    assert_eq!(picture.pixel(200, 99), Some([0.0; 4]));
}

/// Верхний непрозрачный пиксель столбца `x` картинки (верх линии) и его цвет.
fn top_pixel(picture: &ScopePicture, x: u32) -> (u32, [f32; 4]) {
    (0..picture.size().height())
        .map(|y| (y, picture.pixel(x, y).unwrap()))
        .find(|(_, pixel)| pixel[3] > 0.99)
        .expect("у столбца есть линия верха")
}

#[test]
fn the_fill_is_translucent_so_the_picture_shows_through_it() {
    // Решение 337: «сплошная светло-серая масса» закрывала картинку. Глубоко внутри заливки видно картинку.
    let settings = look_settings(|_| {});
    let picture = picture_of(
        &flat(200, 100, [512; 3]),
        HistogramMode::Rgb,
        &settings,
        size(1024, 100),
    );
    let deep = picture.pixel(512, 90).unwrap();
    assert!(
        (deep[3] - settings.render.histogram_fill_opacity).abs() < 1e-6,
        "альфа заливки = histogram_fill_opacity, а не 1: {deep:?}"
    );
    assert!(deep[3] < 0.5);
}

#[test]
fn the_pixel_on_the_curve_top_has_the_full_channel_colour_and_is_opaque() {
    let settings = look_settings(|_| {});
    let picture = picture_of(
        &flat(200, 100, [1023, 0, 0]),
        HistogramMode::Parade,
        &settings,
        size(1024, 300),
    );
    // Правый конец верхней (красной) полосы: линия верха — цвет красного канала, непрозрачная.
    let (row, top) = top_pixel(&picture, 1022);
    assert!(row < 3, "линия на верху столбика полной высоты, строка {row}");
    if row > 0 {
        assert_eq!(
            picture.pixel(1022, row - 1).map(|pixel| pixel[3]),
            Some(0.0),
            "над линией пусто"
        );
    }
    assert_eq!(top[3], 1.0);
    for channel in 0..3 {
        assert!(
            (top[channel] - settings.render.parade_colors[0][channel]).abs() < 1e-5,
            "{top:?}"
        );
    }
}

#[test]
fn a_wide_fill_is_the_channel_colour_darkened_by_the_tint_at_the_fill_opacity() {
    // Равные столбики по всем уровням: заливка широкая, склонов рядом нет — внутри только заливка.
    let settings = look_settings(|render| render.histogram_height = HistogramHeight::LinearToMean { multiple: 3.0 });
    let ramp = picture(1024, 4, |x, _| [x as u16; 3]);
    let picture = picture_of(&ramp, HistogramMode::Parade, &settings, size(1024, 300));
    let tint = settings.render.histogram_fill_tint;
    for (strip, color) in settings.render.parade_colors.iter().enumerate() {
        // Три строки над базовой линией полосы (низ полосы — около строки 97 · (strip + 1) с зазором).
        let strips = scope_model::HistogramStrips::new(3, settings.render.histogram_strip_gap);
        let baseline_row = (strips.normalized_span(strip as u32).1 * 300.0) as u32 - 4;
        let fill = picture.pixel(512, baseline_row).unwrap();
        assert!(
            (fill[3] - settings.render.histogram_fill_opacity).abs() < 1e-6,
            "полоса {strip}: {fill:?}"
        );
        for channel in 0..3 {
            assert!(
                (fill[channel] - color[channel] * tint).abs() < 1e-5,
                "полоса {strip}: {fill:?}"
            );
        }
    }
}

#[test]
fn a_steep_flank_shows_the_edge_line_beside_the_bar_not_only_over_its_top() {
    // Без смягчения столбик серого кадра — ровно два столбца (512, 513) во всю высоту, соседние пусты: вертикальный обрыв.
    let settings = look_settings(|render| render.trace_softening_px = 0.0);
    let picture = picture_of(
        &flat(200, 100, [512; 3]),
        HistogramMode::Rgb,
        &settings,
        size(1024, 100),
    );
    assert_eq!(
        picture.pixel(514, 50).map(|pixel| pixel[3] > 0.5),
        Some(true),
        "пиксель рядом со склоном"
    );
    assert_eq!(
        picture.pixel(511, 50).map(|pixel| pixel[3] > 0.5),
        Some(true),
        "и с другой стороны"
    );
    assert_eq!(
        picture.pixel(516, 50),
        Some([0.0; 4]),
        "в двух столбцах от обрыва пусто"
    );
    // Без линии (толщина 0) склон пуст: пиксель рядом с обрывом принадлежит только линии.
    let without_edge = look_settings(|render| {
        render.trace_softening_px = 0.0;
        render.histogram_edge_width_px = 0.0;
    });
    let picture = picture_of(
        &flat(200, 100, [512; 3]),
        HistogramMode::Rgb,
        &without_edge,
        size(1024, 100),
    );
    assert_eq!(picture.pixel(514, 50), Some([0.0; 4]));
}

#[test]
fn overlaid_identical_channels_give_a_white_edge_and_a_neutral_translucent_fill_not_a_white_mass() {
    let settings = look_settings(|_| {});
    let picture = picture_of(
        &flat(200, 100, [512; 3]),
        HistogramMode::Rgb,
        &settings,
        size(1024, 100),
    );
    let (_, edge) = top_pixel(&picture, 512);
    for channel in 0..3 {
        assert!(
            (edge[channel] - 1.0).abs() < 1e-5,
            "линии трёх каналов совпали — белая: {edge:?}"
        );
    }
    let fill = picture.pixel(512, 90).unwrap();
    assert!(
        (fill[0] - fill[1]).abs() < 1e-5 && (fill[1] - fill[2]).abs() < 1e-5,
        "заливка нейтральная: {fill:?}"
    );
    assert!(
        (fill[3] - settings.render.histogram_fill_opacity).abs() < 1e-6,
        "и полупрозрачная: {fill:?}"
    );
    assert!(
        fill[0] < 0.9,
        "заливка затемнена множителем fill_tint, а не белая масса: {fill:?}"
    );
}

#[test]
fn overlaid_edges_keep_their_own_colours_where_the_channels_differ() {
    let settings = look_settings(|_| {});
    // Красный кадр: красная линия справа, у зелёного и синего линии слева складываются в голубую.
    let picture = picture_of(
        &flat(200, 100, [1023, 0, 0]),
        HistogramMode::Rgb,
        &settings,
        size(1024, 100),
    );
    let primaries = settings.render.rgb_waveform_palette.primaries();
    let (_, right) = top_pixel(&picture, 1020);
    let (_, left) = top_pixel(&picture, 3);
    for channel in 0..3 {
        assert!((right[channel] - primaries[0][channel]).abs() < 1e-5, "{right:?}");
        assert!(
            (left[channel] - (primaries[1][channel] + primaries[2][channel]).min(1.0)).abs() < 1e-5,
            "{left:?}"
        );
    }
}

#[test]
fn changing_the_fill_opacity_changes_only_the_fill_not_the_edge() {
    let faint = look_settings(|render| render.histogram_fill_opacity = 0.2);
    let dense = look_settings(|render| render.histogram_fill_opacity = 0.6);
    let shown = flat(200, 100, [512; 3]);
    let first = picture_of(&shown, HistogramMode::Parade, &faint, size(1024, 300));
    let second = picture_of(&shown, HistogramMode::Parade, &dense, size(1024, 300));
    let (mut edge_pixels, mut fill_pixels) = (0, 0);
    for y in 0..300 {
        for x in 0..1024 {
            let (a, b) = (first.pixel(x, y).unwrap(), second.pixel(x, y).unwrap());
            if a[3] == 1.0 {
                edge_pixels += 1;
                assert_eq!(a, b, "линия не зависит от заливки: ({x}, {y})");
            } else if (a[3] - 0.2).abs() < 1e-6 {
                fill_pixels += 1;
                assert!((b[3] - 0.6).abs() < 1e-6, "заливка втрое плотнее: ({x}, {y})");
            }
        }
    }
    assert!(edge_pixels > 0 && fill_pixels > 0, "{edge_pixels} {fill_pixels}");
}

#[test]
fn the_edge_width_setting_widens_the_line() {
    let top_rows = |width: f32| {
        let settings = look_settings(|render| render.histogram_edge_width_px = width);
        let picture = picture_of(
            &flat(200, 100, [512; 3]),
            HistogramMode::Luma,
            &settings,
            size(1024, 200),
        );
        (0..200).filter(|&y| picture.pixel(512, y).unwrap()[3] > 0.99).count()
    };
    let (thin, thick) = (top_rows(1.0), top_rows(3.0));
    assert!(thick > thin, "{thick} против {thin}");
}

#[test]
fn bar_heights_follow_the_height_scale_variants() {
    // Все коды поровну: по 2 отсчёта на уровень, средний счёт = счёту каждого уровня. Внутри картинки столбики равны, и
    // высота каждого — прямое значение кривой при опоре «кратное × среднее».
    let picture_size = size(1024, 200);
    let shown = picture(1024, 4, |x, _| [x as u16; 3]);
    let measure = |height: HistogramHeight| {
        let settings = settings_with_height(height);
        let picture = picture_of(&shown, HistogramMode::Rgb, &settings, picture_size);
        bar_height_px(&picture, 512)
    };
    let cases = [
        (HistogramHeight::LinearToMean { multiple: 2.0 }, 100.0),
        (
            HistogramHeight::SquareRootToMean { multiple: 2.0 },
            200.0 * 0.5_f32.sqrt(),
        ),
        (
            HistogramHeight::Log2ToMean { multiple: 2.0 },
            200.0 * 3.0_f32.log2() / 5.0_f32.log2(),
        ),
        (HistogramHeight::LinearToMean { multiple: 8.0 }, 25.0),
        (HistogramHeight::LinearToPeak, 200.0),
        (HistogramHeight::SquareRootToPeak, 200.0),
        (HistogramHeight::Log2ToPeak, 200.0),
        (HistogramHeight::LinearToMean { multiple: 0.5 }, 200.0),
    ];
    for (height, expected) in cases {
        let measured = measure(height);
        assert!(
            (measured - expected).abs() < 0.6,
            "{height:?}: высота {measured}, ожидалось {expected}"
        );
    }
}

#[test]
fn a_small_bar_next_to_a_tall_one_is_ranked_by_the_curve() {
    // 90 % отсчётов — серый 200, 10 % — серый 800: счёт малого столбика в девять раз меньше большого.
    let picture_size = size(1024, 200);
    let shown = picture(1000, 4, |x, _| if x < 900 { [200; 3] } else { [800; 3] });
    let measure = |height: HistogramHeight| {
        let settings = settings_with_height(height);
        let picture = picture_of(&shown, HistogramMode::Rgb, &settings, picture_size);
        // Центры столбцов уровней 100 и 400 — x = 200 / 201 и 800 / 801 (оба пикселя в полпикселя от центра клетки).
        (bar_height_px(&picture, 200), bar_height_px(&picture, 800))
    };
    let (tall, small_linear) = measure(HistogramHeight::LinearToPeak);
    assert!((tall - 200.0).abs() < 0.01, "высокий достаёт до верха: {tall}");
    assert!(
        (small_linear - 200.0 / 9.0).abs() < 0.3,
        "линейно в девять раз ниже: {small_linear}"
    );
    let (_, small_root) = measure(HistogramHeight::SquareRootToPeak);
    assert!(
        (small_root - 200.0 / 3.0).abs() < 0.6,
        "корень — втрое ниже: {small_root}"
    );
    let (_, small_log) = measure(HistogramHeight::Log2ToPeak);
    assert!(
        small_log > small_root && small_root > small_linear,
        "лог {small_log} > корень {small_root} > линейно {small_linear}"
    );
}

#[test]
fn the_top_edge_of_a_bar_is_one_pixel_soft() {
    // Высота 66,67 px: ровно один пограничный пиксель со своим покрытием, ниже — полное, выше — ничего.
    let settings = settings_with_height(HistogramHeight::LinearToMean { multiple: 3.0 });
    let picture = picture_of(
        &picture(1024, 4, |x, _| [x as u16; 3]),
        HistogramMode::Rgb,
        &settings,
        size(1024, 200),
    );
    let coverage: Vec<f32> = (0..200).map(|y| picture.pixel(512, y).unwrap()[3]).collect();
    let partial: Vec<f32> = coverage
        .iter()
        .copied()
        .filter(|value| *value > 0.001 && *value < 0.999)
        .collect();
    assert_eq!(partial.len(), 1, "один мягкий пиксель: {partial:?}");
    assert!(
        coverage.windows(2).all(|pair| pair[0] <= pair[1]),
        "покрытие растёт сверху вниз без провалов"
    );
    assert!((coverage.iter().sum::<f32>() - 200.0 / 3.0).abs() < 0.6);
}

#[test]
fn paints_and_planes_follow_the_mode() {
    let settings = ScopeModelSettings::default();
    let paint_of = |mode| resolve(mode, ScopeScan::Upright, &settings).trace_paint();
    assert_eq!(paint_of(HistogramMode::Rgb), TracePaint::HistogramOverlaid);
    assert_eq!(paint_of(HistogramMode::Parade), TracePaint::HistogramSections);
    assert_eq!(paint_of(HistogramMode::Luma), TracePaint::HistogramSections);
    let scope = resolve(HistogramMode::Luma, ScopeScan::Upright, &settings);
    assert_eq!(scope.kind(), ScopeKind::Histogram);
    let counters = counters_of(&flat(40, 40, [5; 3]), &scope, &settings);
    assert_eq!(counters.shape().planes(), 1);
}

/// Вертикальные линии сетки режима: (x, верх, низ, полуширина) каждой.
fn vertical_lines(mode: HistogramMode, settings: &ScopeModelSettings) -> Vec<(f32, f32, f32, f32)> {
    resolve(mode, ScopeScan::Upright, settings)
        .graticule(&settings.render)
        .iter()
        .filter_map(|primitive| match primitive {
            GraticulePrimitive::Segment {
                from,
                to,
                half_width_px,
            } if from[0] == to[0] => Some((from[0], from[1], to[1], *half_width_px)),
            GraticulePrimitive::Segment { .. } => None,
            other => panic!("у гистограммы только отрезки: {other:?}"),
        })
        .collect()
}

#[test]
fn the_grid_has_eleven_vertical_lines_every_ten_percent_standing_at_the_gray_levels() {
    let settings = ScopeModelSettings::default();
    let lines = vertical_lines(HistogramMode::Rgb, &settings);
    assert_eq!(lines.len(), 11, "0, 10, …, 100 %");
    for (step, (x, top, bottom, _)) in lines.iter().enumerate() {
        // Линия стоит в центре столбца, куда правило кладёт серый код этого уровня: код = round(step / 10 · 1023).
        let code = (step as f32 / 10.0 * 1023.0).round() as u16;
        let wanted = (level_of_code(code) as f32 + 0.5) / BINS as f32;
        assert!((x - wanted).abs() < 1e-6, "линия {step}: {x} против {wanted}");
        assert!(
            (x - step as f32 / 10.0).abs() < 0.005,
            "и это примерно {step} · 10 % ширины: {x}"
        );
        assert_eq!((*top, *bottom), (0.0, 1.0), "одна полоса — на всю высоту");
    }
    assert_eq!(
        vertical_lines(HistogramMode::Luma, &settings),
        lines,
        "у яркости та же шкала"
    );
    assert!(
        ScopeKind::Histogram.level_lines().is_empty(),
        "горизонтальных линий уровня у гистограммы нет"
    );
}

#[test]
fn the_fifty_percent_line_is_thicker_than_the_rest() {
    let settings = ScopeModelSettings::default();
    let lines = vertical_lines(HistogramMode::Rgb, &settings);
    for (step, (_, _, _, half_width)) in lines.iter().enumerate() {
        let wanted = if step == 5 {
            settings.render.histogram_midline_half_width_px
        } else {
            settings.render.graticule_half_width_px
        };
        assert_eq!(*half_width, wanted, "линия {step}");
    }
    assert!(settings.render.histogram_midline_half_width_px > settings.render.graticule_half_width_px);
}

#[test]
fn the_parade_grid_repeats_the_scale_in_every_strip_and_separates_the_strips_with_baselines() {
    let settings = ScopeModelSettings::default();
    let scope = resolve(HistogramMode::Parade, ScopeScan::Upright, &settings);
    let grid = scope.graticule(&settings.render);
    assert!(
        grid.len() <= scope_model::graticule::MAX_GRATICULE_PRIMITIVES,
        "{}",
        grid.len()
    );
    let lines = vertical_lines(HistogramMode::Parade, &settings);
    assert_eq!(lines.len(), 33, "11 линий в каждой из трёх полос");
    let strips = scope_model::HistogramStrips::new(3, settings.render.histogram_strip_gap);
    for (index, (_, top, bottom, _)) in lines.iter().enumerate() {
        let (wanted_top, wanted_bottom) = strips.normalized_span(index as u32 / 11);
        assert!(
            (top - wanted_top).abs() < 1e-6 && (bottom - wanted_bottom).abs() < 1e-6,
            "линия {index}"
        );
    }
    // x шкалы одни и те же во всех полосах.
    let columns: Vec<f32> = lines.iter().map(|line| line.0).collect();
    assert_eq!(columns[..11], columns[11..22]);
    assert_eq!(columns[..11], columns[22..]);
    let baselines: Vec<f32> = grid
        .iter()
        .filter_map(|primitive| match primitive {
            GraticulePrimitive::Segment { from, to, .. } if from[1] == to[1] => {
                assert_eq!((from[0], to[0]), (0.0, 1.0), "базовая линия на всю ширину");
                Some(from[1])
            }
            _ => None,
        })
        .collect();
    assert_eq!(baselines.len(), 3);
    for (strip, baseline) in baselines.iter().enumerate() {
        assert!((baseline - strips.normalized_span(strip as u32).1).abs() < 1e-6);
    }
}

#[test]
fn the_histogram_settings_are_validated_and_defaulted() {
    let defaults = ScopeModelSettings::default();
    assert_eq!(defaults.histogram_bins, 512);
    assert_eq!(defaults.histogram_sample_stride.get(), 2);
    assert_eq!(defaults.validate(), Ok(()));
    for bins in [0, 1, 1025] {
        let bad = ScopeModelSettings {
            histogram_bins: bins,
            ..ScopeModelSettings::default()
        };
        assert_eq!(
            bad.validate(),
            Err(scope_model::ScopeSettingsError::HistogramBinsOutOfRange { bins })
        );
    }
    // Уровни из настроек доходят до сетки счётчиков.
    let wide = ScopeModelSettings {
        histogram_bins: 1024,
        ..ScopeModelSettings::default()
    };
    let scope = resolve(HistogramMode::Rgb, ScopeScan::Upright, &wide);
    let counters = counters_of(&flat(64, 64, [1023; 3]), &scope, &wide);
    assert_eq!(counters.shape().columns(), 1024);
    assert_eq!(
        counters.count(0, 0, 1023),
        Some(scope.planned_samples(64, 64, &wide).total() as u32)
    );
}

#[test]
fn a_frame_without_samples_draws_nothing_and_does_not_fail() {
    // Окно меньше шага прореживания: отсчётов нет, опора шкалы нулевая — столбиков нет (а не деление на ноль).
    let settings = ScopeModelSettings {
        histogram_sample_stride: scope_model::SampleStride::try_new(64).unwrap(),
        ..settings_with_height(HistogramHeight::LinearToMean { multiple: 2.0 })
    };
    for mode in [HistogramMode::Rgb, HistogramMode::Parade, HistogramMode::Luma] {
        let picture = picture_of(&flat(1, 1, [500; 3]), mode, &settings, size(64, 32));
        assert!(picture.pixels().iter().all(|pixel| *pixel == [0.0; 4]), "{mode:?}");
    }
}
