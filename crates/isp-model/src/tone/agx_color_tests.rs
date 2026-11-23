//! Основа AgX на цветах: «notorious six», цель решения 47, цвета за охватом, оттенок на колесе.
//!
//! Метрика — OKLCh (Ottosson, `crate::color::oklab`) на ВЫХОДЕ цепочки в sRGB. Свет цвета сцены задаётся
//! цветностью, нормированной на яркость `Y = 1`, и умножается на `scene_middle_gray · 2^e`: при `e = 0` пятно
//! этого цвета имеет яркость среднего серого. Вход не обрезан сенсором (идеальная HDR-сцена); белый точки `L = 1`.
//! Оттенок считается только там, где цветность выхода `C ≥ 0,04` (ниже — цвет почти серый, оттенок —
//! шум); порог один для всех цветов и в тестах не подгонялся.
//!
//! Источник цветов ColorChecker Classic (2005): значения sRGB 8 бит из публикации BabelColor / X-Rite —
//! Dark skin… здесь используются «Light skin» (194, 150, 130), «Blue sky» (98, 122, 157), «Orange» (214, 126, 44).
//! Закат — (255, 120, 20) sRGB, насыщенный оранжевый. R, G, B, C, M, Y — вершины куба sRGB.

use std::sync::Arc;

use crate::color::chromaticity::Primaries;
use crate::color::matrix::{Matrix3, PixelMatrix};
use crate::color::oklab::{Oklch, hue_difference_degrees, linear_rgb_to_oklch};
use crate::color::working_space::WORKING_PRIMARIES;
use crate::image::DisplayGamut;
use crate::tone::agx::AgxBase;
use crate::tone::agx_settings::AgxSettings;
use crate::tone::working_io::WorkingInputGamut;

/// Цветность ниже которой оттенок не оценивается.
const HUE_CHROMA_FLOOR: f64 = 0.04;
const EXPOSURE_STEP_EV: f64 = 0.05;

fn srgb_decode(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn from_srgb8(red: f64, green: f64, blue: f64) -> [f64; 3] {
    [red, green, blue].map(|channel| srgb_decode(channel / 255.0))
}

/// Цвет, нормированный на яркость Y = 1 (линейный sRGB).
fn unit_luminance(color: [f64; 3]) -> [f64; 3] {
    let luminance = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
    color.map(|channel| channel / luminance)
}

struct Rig {
    base: Arc<AgxBase>,
    settings: AgxSettings,
    srgb_to_xyz: Matrix3,
    srgb_to_working: PixelMatrix,
}

impl Rig {
    fn new(settings: AgxSettings) -> Self {
        let srgb_to_xyz = Primaries::SRGB.rgb_to_xyz().unwrap();
        let srgb_to_working = WORKING_PRIMARIES
            .rgb_to_xyz()
            .unwrap()
            .inverse()
            .unwrap()
            .mul(&srgb_to_xyz)
            .to_f32();
        Self {
            base: AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap(),
            settings,
            srgb_to_xyz,
            srgb_to_working,
        }
    }

    /// Основа без возврата оттенка (степень 0, как умолчание этого пакета; явно — чтобы переключение умолчания
    /// на 1 не меняло тесты «чистого» AgX).
    fn default_rig() -> Self {
        Self::new(AgxSettings {
            hue_restoration: 0.0,
            ..AgxSettings::default()
        })
    }

    /// Основа с полным возвратом оттенка (решение 180).
    fn restored_rig() -> Self {
        Self::new(AgxSettings {
            hue_restoration: 1.0,
            ..AgxSettings::default()
        })
    }

    /// Выход цепочки в OKLCh для света сцены `unit_color · grey · 2^e` (линейный sRGB).
    fn output_at(&self, unit_color: [f64; 3], e: f64) -> Oklch {
        let scale = f64::from(self.settings.scene_middle_gray) * e.exp2();
        let scene = unit_color.map(|channel| (channel * scale) as f32);
        let working = self.srgb_to_working.apply(scene);
        let output = self.base.to_output(self.base.map_scene_pixel(working, 1.0));
        linear_rgb_to_oklch(&self.srgb_to_xyz, output)
    }

    fn trajectory(&self, unit_color: [f64; 3], last_ev: f64) -> Vec<Oklch> {
        let steps = (last_ev / EXPOSURE_STEP_EV).round() as usize;
        (0..=steps)
            .map(|step| self.output_at(unit_color, step as f64 * EXPOSURE_STEP_EV))
            .collect()
    }
}

/// Наибольший сдвиг оттенка от e = 0 там, где цвет ещё цветной.
fn max_hue_shift(path: &[Oklch]) -> f64 {
    let start = path[0].hue_degrees;
    path.iter()
        .filter(|point| point.chroma >= HUE_CHROMA_FLOOR)
        .map(|point| hue_difference_degrees(point.hue_degrees, start))
        .fold(0.0, f64::max)
}

/// Наибольший скачок оттенка между соседними шагами (там, где цвет цветной на обоих).
fn max_hue_step(path: &[Oklch]) -> f64 {
    path.windows(2)
        .filter(|pair| pair[0].chroma >= HUE_CHROMA_FLOOR && pair[1].chroma >= HUE_CHROMA_FLOOR)
        .map(|pair| hue_difference_degrees(pair[0].hue_degrees, pair[1].hue_degrees))
        .fold(0.0, f64::max)
}

fn lightness_steps(path: &[Oklch]) -> Vec<f64> {
    path.windows(2)
        .map(|pair| pair[1].lightness - pair[0].lightness)
        .collect()
}

const SIX: [(&str, [f64; 3]); 6] = [
    ("R", [1.0, 0.0, 0.0]),
    ("G", [0.0, 1.0, 0.0]),
    ("B", [0.0, 0.0, 1.0]),
    ("C", [0.0, 1.0, 1.0]),
    ("M", [1.0, 0.0, 1.0]),
    ("Y", [1.0, 1.0, 0.0]),
];

#[test]
fn notorious_six_go_to_white_without_steps() {
    let rig = Rig::default_rig();
    for (name, color) in SIX {
        // До +6 ступеней: белый точки (L = 1) наступает на 3,74-й; чистому зелёному, у которого другие каналы
        // почти пусты, до белого нужно дольше остальных.
        let path = rig.trajectory(unit_luminance(color), 6.0);
        let steps = lightness_steps(&path);
        // Замер: яркость не убывает, шаг по яркости ≤ 0,009 на 0,05 EV, оттенок ≤ 2,7° на шаг.
        // Потолки — с запасом около 2,5 раза: ступенька или излом кривой даёт кратно больше.
        assert!(steps.iter().all(|step| *step >= -1e-6), "{name}: яркость убывает");
        let max_step = steps.iter().copied().fold(0.0, f64::max);
        assert!(max_step < 0.02, "{name}: шаг по яркости {max_step}");
        let hue_step = max_hue_step(&path);
        assert!(hue_step < 4.0, "{name}: скачок оттенка {hue_step}°");
        let last = path.last().unwrap();
        assert!(
            last.chroma < 0.02 && last.lightness > 0.985,
            "{name}: в конце не белый: {last:?}"
        );
    }
}

#[test]
fn skin_and_sky_keep_their_hue_within_the_decision_47_ceilings() {
    let rig = Rig::default_rig();
    let skin = max_hue_shift(&rig.trajectory(unit_luminance(from_srgb8(194.0, 150.0, 130.0)), 4.0));
    let sky = max_hue_shift(&rig.trajectory(unit_luminance(from_srgb8(98.0, 122.0, 157.0)), 4.0));
    // Замер: кожа 1,2°, небо 0,8° — потолки решения 47 (абсолютные, не подгонялись).
    assert!(skin <= 4.0, "кожа сдвинулась на {skin}°");
    assert!(sky <= 6.0, "небо сдвинулось на {sky}°");
}

const ORANGE: [f64; 3] = [214.0, 126.0, 44.0];
const SUNSET: [f64; 3] = [255.0, 120.0, 20.0];

#[test]
fn orange_and_sunset_hue_shift_does_not_regress_from_the_measurement() {
    // Замер на стартовых параметрах: оранжевый 10,2°, закат 5,5°; прежняя основа — 10,6° и 10,3°.
    // Это страховка от ухудшения, а НЕ принятый потолок решения 47 (он — в тесте выше).
    let rig = Rig::default_rig();
    let orange = max_hue_shift(&rig.trajectory(unit_luminance(from_srgb8(214.0, 126.0, 44.0)), 4.0));
    let sunset = max_hue_shift(&rig.trajectory(unit_luminance(from_srgb8(255.0, 120.0, 20.0)), 4.0));
    assert!(orange > 4.0 && orange < 12.5, "оранжевый {orange}°");
    assert!(sunset < 7.0, "закат {sunset}°");
}

fn hsv_to_srgb8(hue_sector: f64, saturation: f64, value: f64) -> [f64; 3] {
    let index = hue_sector.floor();
    let fraction = hue_sector - index;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - saturation * fraction);
    let t = value * (1.0 - saturation * (1.0 - fraction));
    let (red, green, blue) = match index as i64 % 6 {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    };
    [red * 255.0, green * 255.0, blue * 255.0]
}

#[test]
fn hue_wheel_keeps_hue_around_middle_gray() {
    // 24 оттенка умеренной насыщенности на e = −1, 0, +1: матрицы «внутрь» и «наружу» не должны крутить цвета.
    // Замер: наибольший сдвиг 6,9° (подбор параметров намеренно ограничен этим же колесом — иначе поворот
    // оттенка в углах нужен орбите оранжевого, но разворачивает розовое полотенце в оливковое).
    let rig = Rig::default_rig();
    let mut worst: f64 = 0.0;
    for step in 0..24 {
        let [red, green, blue] = hsv_to_srgb8(f64::from(step) * 0.25, 0.5, 0.6);
        let color = unit_luminance(from_srgb8(red, green, blue));
        let input = linear_rgb_to_oklch(&rig.srgb_to_xyz, color.map(|channel| channel as f32));
        for e in [-1.0, 0.0, 1.0] {
            let output = rig.output_at(color, e);
            worst = worst.max(hue_difference_degrees(output.hue_degrees, input.hue_degrees));
        }
    }
    assert!(worst < 8.0, "наибольший сдвиг оттенка на колесе {worst}°");
}

/// Цветности спектральных и пурпурных цветов за охватом Rec. 2020 (CIE 1931 xy; спектральный локус по таблице CIE).
const BEYOND_REC2020: [(&str, f64, f64); 7] = [
    ("405 нм", 0.1566, 0.0177),
    ("470 нм", 0.1241, 0.0578),
    ("480 нм", 0.0913, 0.1327),
    ("490 нм", 0.0454, 0.2950),
    ("500 нм", 0.0082, 0.5384),
    ("пурпурный", 0.25, 0.06),
    ("глубокий красный", 0.735, 0.265),
];

fn beyond_gamut_pass(input_gamut: WorkingInputGamut) {
    let settings = AgxSettings {
        input_gamut,
        ..AgxSettings::default()
    };
    let rig = Rig::new(settings);
    let xyz_to_working = WORKING_PRIMARIES.rgb_to_xyz().unwrap().inverse().unwrap().to_f32();
    let white = (0.3127_f64, 0.3290_f64);
    for (name, x, y) in BEYOND_REC2020 {
        // Путь по насыщенности от белой точки к спектральному цвету при яркости среднего серого.
        let mut previous_hue: Option<f64> = None;
        for step in 0..=100 {
            let t = f64::from(step) / 100.0;
            let (cx, cy) = (white.0 + (x - white.0) * t, white.1 + (y - white.1) * t);
            let luminance = f64::from(rig.settings.scene_middle_gray);
            let xyz = [cx / cy * luminance, luminance, (1.0 - cx - cy) / cy * luminance];
            let working = xyz_to_working.apply(xyz.map(|c| c as f32));
            let display = rig.base.map_scene_pixel(working, 1.0);
            let output = rig.base.to_output(display);
            assert!(display.iter().all(|c| c.is_finite()), "{name} t={t}: {display:?}");
            assert!(
                output.iter().all(|c| (0.0..=1.0).contains(c)),
                "{name} t={t}: {output:?}"
            );
            let lch = linear_rgb_to_oklch(&rig.srgb_to_xyz, output);
            if lch.chroma >= HUE_CHROMA_FLOOR {
                if let Some(previous) = previous_hue {
                    let jump = hue_difference_degrees(lch.hue_degrees, previous);
                    // Замер: наибольший скачок на шаге насыщенности 0,01 — 2,2°.
                    assert!(jump < 4.0, "{name} t={t}: скачок оттенка {jump}°");
                }
                previous_hue = Some(lch.hue_degrees);
            }
        }
        // Путь по яркости при полной насыщенности: без ступенек и без убывания.
        let mut previous_lightness: Option<f64> = None;
        for step in 0..=160 {
            let e = -4.0 + f64::from(step) * EXPOSURE_STEP_EV;
            let luminance = f64::from(rig.settings.scene_middle_gray) * e.exp2();
            let xyz = [x / y * luminance, luminance, (1.0 - x - y) / y * luminance];
            let working = xyz_to_working.apply(xyz.map(|c| c as f32));
            let output = rig.base.to_output(rig.base.map_scene_pixel(working, 1.0));
            let lightness = linear_rgb_to_oklch(&rig.srgb_to_xyz, output).lightness;
            if let Some(previous) = previous_lightness {
                // Замер: шаг ≤ 0,008 на 0,05 EV.
                assert!(lightness - previous >= -1e-6, "{name} e={e}: яркость убывает");
                assert!(lightness - previous < 0.02, "{name} e={e}: ступенька яркости");
            }
            previous_lightness = Some(lightness);
        }
    }
}

#[test]
fn colors_beyond_rec2020_are_safe_with_clip_to_zero_input() {
    beyond_gamut_pass(WorkingInputGamut::ClipToZero);
}

#[test]
fn colors_beyond_rec2020_are_safe_with_soft_compress_input() {
    beyond_gamut_pass(WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION));
}

// ---- Возврат оттенка (решение 180) при степени 1: потолки приёмки решений 47 и 181 ----

fn assert_shift_within(name: &str, color: [f64; 3], ceiling: f64) {
    let rig = Rig::restored_rig();
    let shift = max_hue_shift(&rig.trajectory(unit_luminance(from_srgb8(color[0], color[1], color[2])), 4.0));
    assert!(
        shift <= ceiling,
        "{name}: сдвиг оттенка {shift}° выше потолка {ceiling}°"
    );
}

#[test]
fn restored_orange_and_sunset_keep_their_hue_within_the_decision_181_ceilings() {
    // Потолки решения 181: оранжевый ≤ 4°, закат ≤ 5° (от e = 0 до +4 EV, OKLCh, выход sRGB).
    assert_shift_within("оранжевый", ORANGE, 4.0);
    assert_shift_within("закат", SUNSET, 5.0);
}

#[test]
fn restored_skin_and_sky_keep_their_hue_within_the_decision_181_ceilings() {
    assert_shift_within("кожа", [194.0, 150.0, 130.0], 4.0);
    assert_shift_within("небо", [98.0, 122.0, 157.0], 6.0);
}

#[test]
fn restored_notorious_six_go_to_white_smoothly_and_monotonically() {
    let rig = Rig::restored_rig();
    for (name, color) in SIX {
        let path = rig.trajectory(unit_luminance(color), 6.0);
        // Гладкость пути: скачок оттенка на шаге 0,05 EV (замер решения 180 — до 0,7°; потолок с запасом).
        let hue_step = max_hue_step(&path);
        assert!(hue_step < 3.0, "{name}: скачок оттенка {hue_step}°");
        let steps = lightness_steps(&path);
        assert!(steps.iter().all(|step| *step >= -1e-6), "{name}: яркость убывает");
        assert!(
            steps.iter().copied().fold(0.0, f64::max) < 0.02,
            "{name}: ступенька яркости"
        );
        // Уход в белое не ломается: от +3,5 до +6 EV цветность монотонно убывает к нулю.
        let from = (3.5 / EXPOSURE_STEP_EV).round() as usize;
        for pair in path[from..].windows(2) {
            assert!(
                pair[1].chroma <= pair[0].chroma + 2e-4,
                "{name}: цветность растёт на пути в белое: {} -> {}",
                pair[0].chroma,
                pair[1].chroma
            );
        }
        let last = path.last().unwrap();
        assert!(
            last.chroma < 0.02 && last.lightness > 0.985,
            "{name}: в конце не белый: {last:?}"
        );
    }
}

#[test]
fn restoration_fixes_the_primaries_that_pure_agx_rotates() {
    // Без возврата красный/зелёный/синий уходят на десятки градусов (замер решения 180: 31–45°).
    let pure = Rig::default_rig();
    let restored = Rig::restored_rig();
    for (name, color) in &SIX[..3] {
        let unit = unit_luminance(*color);
        let before = max_hue_shift(&pure.trajectory(unit, 4.0));
        let after = max_hue_shift(&restored.trajectory(unit, 4.0));
        assert!(before > 15.0, "{name}: чистый AgX сдвинул лишь на {before}°");
        assert!(after < 6.0, "{name}: с возвратом сдвиг {after}°");
    }
}

#[test]
fn partial_degree_lies_between_pure_and_full_restoration() {
    let unit = unit_luminance([1.0, 0.0, 0.0]);
    let shift_at = |degree: f32| {
        let rig = Rig::new(AgxSettings {
            hue_restoration: degree,
            ..AgxSettings::default()
        });
        max_hue_shift(&rig.trajectory(unit, 4.0))
    };
    let (none, half, full) = (shift_at(0.0), shift_at(0.5), shift_at(1.0));
    assert!(full < half && half < none, "{none}° > {half}° > {full}°");
}

#[test]
fn degree_zero_skips_the_step_and_matches_the_plain_formula_bit_for_bit() {
    let rig = Rig::default_rig();
    assert!(rig.base.parameters().hue_restoration.is_none());
    let parameters = rig.base.parameters();
    let pixel = [0.31_f32, 0.07, 0.9];
    let inset = crate::color::matrix::PixelMatrix { rows: parameters.inset }.apply(pixel);
    let curved = inset.map(|channel| rig.base.curve_channel(channel, 1.0).powf(parameters.gamma));
    let expected = crate::color::matrix::PixelMatrix {
        rows: parameters.outset,
    }
    .apply(curved);
    assert_eq!(rig.base.map_scene_pixel(pixel, 1.0), expected);
}

#[test]
fn restoration_keeps_grey_grey_and_output_finite_beyond_the_gamut() {
    let rig = Rig::restored_rig();
    for step in 0..120 {
        let level = 0.001 * 1.1_f32.powi(step);
        let display = rig.base.map_scene_pixel([level; 3], 1.0);
        let spread =
            display.iter().copied().fold(f32::MIN, f32::max) - display.iter().copied().fold(f32::MAX, f32::min);
        assert!(spread < 2e-5, "серый {level} окрасился: {display:?}");
    }
    for pixel in [
        [-0.2_f32, 0.1, 0.4],
        [3.0, -1.0, 0.0],
        [0.0; 3],
        [1e-30; 3],
        [50.0, 0.0, 0.0],
    ] {
        let display = rig.base.map_scene_pixel(pixel, 1.0);
        assert!(
            display.iter().all(|channel| channel.is_finite()),
            "{pixel:?} -> {display:?}"
        );
    }
}

#[test]
fn beyond_gamut_colours_stay_safe_with_restoration() {
    let settings = AgxSettings {
        hue_restoration: 1.0,
        ..AgxSettings::default()
    };
    let rig = Rig::new(settings);
    let xyz_to_working = WORKING_PRIMARIES.rgb_to_xyz().unwrap().inverse().unwrap().to_f32();
    for (name, x, y) in BEYOND_REC2020 {
        for step in 0..=100 {
            let luminance = f64::from(rig.settings.scene_middle_gray);
            let t = f64::from(step) / 100.0;
            let (cx, cy) = (0.3127 + (x - 0.3127) * t, 0.3290 + (y - 0.3290) * t);
            let xyz = [cx / cy * luminance, luminance, (1.0 - cx - cy) / cy * luminance];
            let working = xyz_to_working.apply(xyz.map(|c| c as f32));
            let display = rig.base.map_scene_pixel(working, 1.0);
            let output = rig.base.to_output(display);
            assert!(display.iter().all(|c| c.is_finite()), "{name} t={t}: {display:?}");
            assert!(
                output.iter().all(|c| (0.0..=1.0).contains(c)),
                "{name} t={t}: {output:?}"
            );
        }
    }
}
