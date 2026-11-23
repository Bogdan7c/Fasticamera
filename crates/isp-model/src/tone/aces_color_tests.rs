//! Основа ACES на цветах: «notorious six», цель решения 47, цвета за охватом, колесо оттенков, цветность и наклон
//! относительно AgX.
//!
//! Метрика — та же, что у AgX (`agx_color_tests`): OKLCh на ВЫХОДЕ цепочки в sRGB; свет цвета сцены — цветность,
//! нормированная на яркость `Y = 1`, умноженная на `scene_middle_gray · 2^e` (при `e = 0` пятно цвета имеет яркость
//! среднего серого); вход не обрезан сенсором, белый точки `L = 1`; оттенок оценивается только там, где цветность
//! выхода `C ≥ 0,04`. Входные цвета — ColorChecker Classic (2005, sRGB 8 бит), закат (255, 120, 20), вершины куба sRGB.

use std::sync::Arc;

use super::AcesBase;
use super::settings::AcesSettings;
use crate::color::chromaticity::Primaries;
use crate::color::matrix::{Matrix3, PixelMatrix};
use crate::color::oklab::{Oklch, hue_difference_degrees, linear_rgb_to_oklch};
use crate::color::working_space::WORKING_PRIMARIES;
use crate::image::DisplayGamut;
use crate::tone::agx::AgxBase;
use crate::tone::agx_settings::AgxSettings;

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

fn unit_luminance(color: [f64; 3]) -> [f64; 3] {
    let luminance = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
    color.map(|channel| channel / luminance)
}

/// Основа тона для измерений: свет сцены → выход sRGB в OKLCh.
trait Tone {
    fn working_to_output(&self, scene_working: [f32; 3]) -> [f32; 3];
}

impl Tone for AcesBase {
    fn working_to_output(&self, scene_working: [f32; 3]) -> [f32; 3] {
        self.to_output(self.map_scene_pixel(scene_working, 1.0))
    }
}

impl Tone for AgxBase {
    fn working_to_output(&self, scene_working: [f32; 3]) -> [f32; 3] {
        self.to_output(self.map_scene_pixel(scene_working, 1.0))
    }
}

struct Rig<B: Tone> {
    base: Arc<B>,
    grey: f64,
    srgb_to_xyz: Matrix3,
    srgb_to_working: PixelMatrix,
}

impl<B: Tone> Rig<B> {
    fn with_base(base: Arc<B>, grey: f64) -> Self {
        let srgb_to_xyz = Primaries::SRGB.rgb_to_xyz().unwrap();
        let srgb_to_working = WORKING_PRIMARIES
            .rgb_to_xyz()
            .unwrap()
            .inverse()
            .unwrap()
            .mul(&srgb_to_xyz)
            .to_f32();
        Self {
            base,
            grey,
            srgb_to_xyz,
            srgb_to_working,
        }
    }

    /// Выход цепочки в OKLCh для света сцены `unit_color · grey · 2^e` (линейный sRGB).
    fn output_at(&self, unit_color: [f64; 3], e: f64) -> Oklch {
        let scale = self.grey * e.exp2();
        let scene = unit_color.map(|channel| (channel * scale) as f32);
        let output = self.base.working_to_output(self.srgb_to_working.apply(scene));
        linear_rgb_to_oklch(&self.srgb_to_xyz, output)
    }

    fn trajectory(&self, unit_color: [f64; 3], last_ev: f64) -> Vec<Oklch> {
        let steps = (last_ev / EXPOSURE_STEP_EV).round() as usize;
        (0..=steps)
            .map(|step| self.output_at(unit_color, step as f64 * EXPOSURE_STEP_EV))
            .collect()
    }
}

fn aces_rig(settings: AcesSettings) -> Rig<AcesBase> {
    let grey = f64::from(settings.scene_middle_gray);
    Rig::with_base(AcesBase::resolve(&settings, DisplayGamut::Srgb).unwrap(), grey)
}

fn default_rig() -> Rig<AcesBase> {
    aces_rig(AcesSettings::default())
}

fn agx_rig() -> Rig<AgxBase> {
    let settings = AgxSettings {
        hue_restoration: 0.0,
        ..AgxSettings::default()
    };
    let grey = f64::from(settings.scene_middle_gray);
    Rig::with_base(AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap(), grey)
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

const SKIN: [f64; 3] = [194.0, 150.0, 130.0];
const SKY: [f64; 3] = [98.0, 122.0, 157.0];
const ORANGE: [f64; 3] = [214.0, 126.0, 44.0];
const SUNSET: [f64; 3] = [255.0, 120.0, 20.0];

fn shift_of<B: Tone>(rig: &Rig<B>, color: [f64; 3]) -> f64 {
    max_hue_shift(&rig.trajectory(unit_luminance(from_srgb8(color[0], color[1], color[2])), 4.0))
}

#[test]
fn notorious_six_go_to_white_without_steps() {
    let rig = default_rig();
    for (name, color) in SIX {
        let path = rig.trajectory(unit_luminance(color), 7.0);
        let steps = lightness_steps(&path);
        assert!(steps.iter().all(|step| *step >= -1e-6), "{name}: яркость убывает");
        let max_step = steps.iter().copied().fold(0.0, f64::max);
        assert!(max_step < MAX_LIGHTNESS_STEP, "{name}: шаг по яркости {max_step}");
        let hue_step = max_hue_step(&path);
        assert!(hue_step < MAX_HUE_STEP_DEGREES, "{name}: скачок оттенка {hue_step}°");
        let last = path.last().unwrap();
        assert!(
            last.chroma < 0.02 && last.lightness > 0.985,
            "{name}: в конце не белый: {last:?}"
        );
    }
}

/// Потолки гладкости пути в белое — те же, что у AgX (`agx_color_tests`); замер ACES: шаг яркости ≤ 0,0084 на
/// 0,05 ступени, скачок оттенка ≤ 1,6°.
const MAX_LIGHTNESS_STEP: f64 = 0.02;
const MAX_HUE_STEP_DEGREES: f64 = 4.0;

#[test]
#[ignore = "печатает таблицу замеров для отчёта: cargo test -p isp-model --lib aces_measurements -- --ignored --nocapture"]
fn aces_measurements() {
    let aces = default_rig();
    let agx = agx_rig();
    let colours: [(&str, [f64; 3]); 10] = [
        ("кожа", SKIN),
        ("небо", SKY),
        ("оранжевый", ORANGE),
        ("закат", SUNSET),
        ("R", [255.0, 0.0, 0.0]),
        ("G", [0.0, 255.0, 0.0]),
        ("B", [0.0, 0.0, 255.0]),
        ("C", [0.0, 255.0, 255.0]),
        ("M", [255.0, 0.0, 255.0]),
        ("Y", [255.0, 255.0, 0.0]),
    ];
    println!("сдвиг оттенка 0…+4 EV, OKLCh, выход sRGB (ACES / AgX):");
    for (name, color) in colours {
        let (a, g) = (shift_of(&aces, color), shift_of(&agx, color));
        println!("  {name:10} {a:6.1}°  {g:6.1}°");
    }
    for (name, color) in SIX {
        let path = aces.trajectory(unit_luminance(color), 7.0);
        let steps = lightness_steps(&path);
        println!(
            "  путь в белое {name}: шаг яркости ≤ {:.4}, скачок оттенка ≤ {:.1}°",
            steps.iter().copied().fold(0.0, f64::max),
            max_hue_step(&path)
        );
    }
    let mut worst: [f64; 2] = [0.0; 2];
    for step in 0..24 {
        let [red, green, blue] = wheel_color(f64::from(step) * 0.25);
        let color = unit_luminance(from_srgb8(red, green, blue));
        let input = linear_rgb_to_oklch(&aces.srgb_to_xyz, color.map(|channel| channel as f32));
        for e in [-1.0, 0.0, 1.0] {
            worst[0] = worst[0].max(hue_difference_degrees(
                aces.output_at(color, e).hue_degrees,
                input.hue_degrees,
            ));
            worst[1] = worst[1].max(hue_difference_degrees(
                agx.output_at(color, e).hue_degrees,
                input.hue_degrees,
            ));
        }
    }
    println!(
        "колесо 24 оттенков на e = −1, 0, +1: ACES {:.1}°, AgX {:.1}°",
        worst[0], worst[1]
    );
    let mut chroma = [0.0_f64; 2];
    let mut count = 0.0;
    for (_, color) in colours {
        let unit = unit_luminance(from_srgb8(color[0], color[1], color[2]));
        for e in [-2.0, -1.0, 0.0, 1.0, 2.0] {
            chroma[0] += aces.output_at(unit, e).chroma;
            chroma[1] += agx.output_at(unit, e).chroma;
            count += 1.0;
        }
    }
    println!(
        "средняя цветность (10 цветов × e −2…+2): ACES {:.4}, AgX {:.4}, отношение {:.3}",
        chroma[0] / count,
        chroma[1] / count,
        chroma[0] / chroma[1]
    );
    let slope = |rig: &Rig<AcesBase>| {
        let low = rig
            .base
            .working_to_output(rig.srgb_to_working.apply([(rig.grey * (-0.05_f64).exp2()) as f32; 3]))[0];
        let high = rig
            .base
            .working_to_output(rig.srgb_to_working.apply([(rig.grey * 0.05_f64.exp2()) as f32; 3]))[0];
        f64::from(high - low) / 0.1
    };
    let agx_slope = {
        let low = agx
            .base
            .working_to_output(agx.srgb_to_working.apply([(agx.grey * (-0.05_f64).exp2()) as f32; 3]))[0];
        let high = agx
            .base
            .working_to_output(agx.srgb_to_working.apply([(agx.grey * 0.05_f64.exp2()) as f32; 3]))[0];
        f64::from(high - low) / 0.1
    };
    println!(
        "наклон в середине (линейный свет экрана на ступень): ACES {:.4}, AgX {agx_slope:.4}",
        slope(&aces)
    );
    let lightness = |rig: &Rig<AcesBase>, e: f64| rig.output_at([1.0; 3], e).lightness;
    println!(
        "светлота OKLab нейтрали (e = −4, −2, 0, +2, +3,7): ACES {:.3} {:.3} {:.3} {:.3} {:.3}",
        lightness(&aces, -4.0),
        lightness(&aces, -2.0),
        lightness(&aces, 0.0),
        lightness(&aces, 2.0),
        lightness(&aces, 3.7)
    );
}

fn wheel_color(hue_sector: f64) -> [f64; 3] {
    let index = hue_sector.floor();
    let fraction = hue_sector - index;
    let (saturation, value) = (0.5, 0.6);
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
#[ignore = "диагностика вкладов шагов ACES в сдвиг оттенка"]
fn aces_variants() {
    let base = AcesSettings::default();
    let variants: [(&str, AcesSettings); 8] = [
        ("умолчание", base),
        ("колено 3,0", AcesSettings { knee_ev: 3.0, ..base }),
        (
            "без красного мод.",
            AcesSettings {
                red_modifier_scale: 1.0,
                ..base
            },
        ),
        ("без свечения", AcesSettings { glow_gain: 0.0, ..base }),
        (
            "без десат. RRT",
            AcesSettings {
                rrt_saturation: 1.0,
                ..base
            },
        ),
        (
            "без десат. ODT",
            AcesSettings {
                odt_saturation: 1.0,
                ..base
            },
        ),
        (
            "без окружения",
            AcesSettings {
                surround_gamma: 1.0,
                ..base
            },
        ),
        (
            "всё выкл.",
            AcesSettings {
                knee_ev: 3.0,
                red_modifier_scale: 1.0,
                glow_gain: 0.0,
                rrt_saturation: 1.0,
                odt_saturation: 1.0,
                surround_gamma: 1.0,
                ..base
            },
        ),
    ];
    for (name, settings) in variants {
        let rig = aces_rig(settings);
        println!(
            "{name:20} кожа {:5.1} небо {:5.1} оранж {:5.1} закат {:5.1} R {:5.1} G {:5.1} B {:5.1}",
            shift_of(&rig, SKIN),
            shift_of(&rig, SKY),
            shift_of(&rig, ORANGE),
            shift_of(&rig, SUNSET),
            shift_of(&rig, [255.0, 0.0, 0.0]),
            shift_of(&rig, [0.0, 255.0, 0.0]),
            shift_of(&rig, [0.0, 0.0, 255.0]),
        );
    }
}

// ---- Цель решений 47 и 181: замер ACES и страховки от ухудшения ----
//
// Потолки решения 181 (от e = 0 до +4 EV, OKLCh, выход sRGB): оранжевый и кожа ≤ 4°, закат ≤ 5°, небо ≤ 6°.
// ACES по каналам их НЕ держит, кроме неба (замер: кожа 7,1°, небо 4,6°, оранжевый 53,2°, закат 66,2°; AgX без
// возврата оттенка — 1,2° / 0,8° / 10,2° / 5,5°). Потолки здесь НЕ ослаблены: тесты ниже — страховка от ухудшения
// измеренного, а решение о том, годится ли ACES как умолчание, принимает владелец в 14b.

#[test]
fn sky_keeps_its_hue_within_the_decision_181_ceiling() {
    // Единственный из четырёх потолков, который ACES держит: небо ≤ 6° (замер 4,6°).
    let sky = shift_of(&default_rig(), SKY);
    assert!(sky <= 6.0, "небо сдвинулось на {sky}°");
}

#[test]
fn skin_orange_and_sunset_hue_shift_does_not_regress_from_the_measurement() {
    let rig = default_rig();
    let (skin, orange, sunset) = (shift_of(&rig, SKIN), shift_of(&rig, ORANGE), shift_of(&rig, SUNSET));
    // Замер: кожа 7,1°, оранжевый 53,2°, закат 66,2° (потолки 4°, 4°, 5° не держатся — см. заметку выше).
    assert!(skin <= 8.0, "кожа {skin}°");
    assert!(orange <= 58.0, "оранжевый {orange}°");
    assert!(sunset <= 72.0, "закат {sunset}°");
    // Страховка от тихого «улучшения» сломанной формулой: у ACES по каналам оранжевый обязан заметно уходить в жёлтое.
    assert!(orange > 20.0 && sunset > 20.0, "оранжевый {orange}°, закат {sunset}°");
}

#[test]
fn hue_wheel_does_not_regress_from_the_measurement() {
    // 24 оттенка умеренной насыщенности на e = −1, 0, +1. Замер: ACES 10,2° (AgX 5,8°).
    let rig = default_rig();
    let mut worst: f64 = 0.0;
    for step in 0..24 {
        let [red, green, blue] = wheel_color(f64::from(step) * 0.25);
        let color = unit_luminance(from_srgb8(red, green, blue));
        let input = linear_rgb_to_oklch(&rig.srgb_to_xyz, color.map(|channel| channel as f32));
        for e in [-1.0, 0.0, 1.0] {
            let output = rig.output_at(color, e);
            worst = worst.max(hue_difference_degrees(output.hue_degrees, input.hue_degrees));
        }
    }
    assert!(worst < 12.0, "наибольший сдвиг оттенка на колесе {worst}°");
}

#[test]
fn chroma_follows_agx_within_the_measured_band_and_falls_to_zero_in_the_white() {
    let (aces, agx) = (default_rig(), agx_rig());
    let mut ratio_sum = 0.0;
    let mut count = 0.0;
    for (_, color) in SIX {
        let unit = unit_luminance(color);
        for e in [-2.0, -1.0, 0.0, 1.0, 2.0] {
            let (a, g) = (aces.output_at(unit, e).chroma, agx.output_at(unit, e).chroma);
            ratio_sum += a / g.max(1e-6);
            count += 1.0;
        }
    }
    let mean_ratio = ratio_sum / count;
    // Замер на 10 цветах: средняя цветность ACES / AgX = 1,045; на шести вершинах куба — вокруг 1.
    assert!((0.7..1.4).contains(&mean_ratio), "цветность ACES к AgX: {mean_ratio}");
    // На пути в белое цветность монотонно убывает к нулю (от +3,5 EV).
    for (name, color) in SIX {
        let path = aces.trajectory(unit_luminance(color), 7.0);
        let from = (3.5 / EXPOSURE_STEP_EV).round() as usize;
        for pair in path[from..].windows(2) {
            assert!(
                pair[1].chroma <= pair[0].chroma + 2e-4,
                "{name}: цветность растёт на пути в белое: {} -> {}",
                pair[0].chroma,
                pair[1].chroma
            );
        }
    }
}

#[test]
fn slope_in_the_middle_is_close_to_agx_by_construction_of_the_display_grey() {
    // Серый обеих основ — 0,20; наклон в нём у ACES — собственный (S-кривая ACES), у AgX — `slope`.
    // Измеряем наклон светлоты OKLab нейтрали на ступень: обе кривые «киношные», отличие в пределах 40 %.
    let (aces, agx) = (default_rig(), agx_rig());
    let slope = |lightness: &dyn Fn(f64) -> f64| (lightness(0.1) - lightness(-0.1)) / 0.2;
    let aces_slope = slope(&|e| aces.output_at([1.0; 3], e).lightness);
    let agx_slope = slope(&|e| agx.output_at([1.0; 3], e).lightness);
    assert!(aces_slope > 0.0 && agx_slope > 0.0);
    let ratio = aces_slope / agx_slope;
    assert!(
        (0.6..1.7).contains(&ratio),
        "наклон светлоты в середине: ACES {aces_slope}, AgX {agx_slope}"
    );
}

// ---- Цвета за охватом Rec. 2020 ----

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

fn beyond_gamut_pass(input_gamut: crate::tone::working_io::WorkingInputGamut) {
    let rig = aces_rig(AcesSettings {
        input_gamut,
        ..AcesSettings::default()
    });
    let xyz_to_working = WORKING_PRIMARIES.rgb_to_xyz().unwrap().inverse().unwrap().to_f32();
    let white = (0.3127_f64, 0.3290_f64);
    let luminance = rig.grey;
    for (name, x, y) in BEYOND_REC2020 {
        let mut previous_hue: Option<f64> = None;
        for step in 0..=100 {
            let t = f64::from(step) / 100.0;
            let (cx, cy) = (white.0 + (x - white.0) * t, white.1 + (y - white.1) * t);
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
                    assert!(
                        jump < BEYOND_GAMUT_HUE_JUMP_DEGREES,
                        "{name} t={t}: скачок оттенка {jump}°"
                    );
                }
                previous_hue = Some(lch.hue_degrees);
            }
        }
        // Путь по яркости при полной насыщенности: без ступенек и без убывания.
        let mut previous_lightness: Option<f64> = None;
        for step in 0..=160 {
            let e = -4.0 + f64::from(step) * EXPOSURE_STEP_EV;
            let luminance = luminance * e.exp2();
            let xyz = [x / y * luminance, luminance, (1.0 - x - y) / y * luminance];
            let working = xyz_to_working.apply(xyz.map(|c| c as f32));
            let output = rig.base.to_output(rig.base.map_scene_pixel(working, 1.0));
            let lightness = linear_rgb_to_oklch(&rig.srgb_to_xyz, output).lightness;
            if let Some(previous) = previous_lightness {
                // Замер: у «пурпурного» за охватом яркость на выходе sRGB может просесть на 1,4·10⁻⁵ (синий канал рабочего
                // пространства насыщается раньше соседних, а в обратной матрице у него отрицательные доли других каналов).
                assert!(lightness - previous >= -1e-4, "{name} e={e}: яркость убывает");
                assert!(lightness - previous < 0.02, "{name} e={e}: ступенька яркости");
            }
            previous_lightness = Some(lightness);
        }
    }
}

/// Наибольший скачок оттенка на шаге насыщенности 0,01 у цветов за охватом (замер ACES ниже; AgX — 2,2°).
const BEYOND_GAMUT_HUE_JUMP_DEGREES: f64 = 6.0;

#[test]
fn colors_beyond_rec2020_are_safe_with_clip_to_zero_input() {
    beyond_gamut_pass(crate::tone::working_io::WorkingInputGamut::ClipToZero);
}

#[test]
fn colors_beyond_rec2020_are_safe_with_soft_compress_input() {
    beyond_gamut_pass(crate::tone::working_io::WorkingInputGamut::SoftCompress(
        crate::tone::working_io::WorkingInputGamut::DEFAULT_COMPRESSION,
    ));
}
