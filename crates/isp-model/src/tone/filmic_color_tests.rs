//! Основа filmic на цветах: «notorious six», цель решения 47, цвета за охватом, колесо оттенков, цветность.
//!
//! Метрика — та же, что у AgX (`agx_color_tests`): OKLCh на ВЫХОДЕ цепочки в sRGB; свет цвета сцены — цветность,
//! нормированная на яркость `Y = 1`, умноженная на `scene_middle_gray · 2^e` (при `e = 0` пятно цвета имеет
//! яркость среднего серого); вход не обрезан сенсором, белый точки `L = 1`; оттенок оценивается только там, где
//! цветность выхода `C ≥ 0,04`. Входные цвета — ColorChecker Classic (2005, sRGB 8 бит), закат (255, 120, 20),
//! вершины куба sRGB — как в `agx_color_tests`.

use std::sync::Arc;

use super::*;
use crate::color::chromaticity::Primaries;
use crate::color::matrix::{Matrix3, PixelMatrix};
use crate::color::oklab::{Oklch, hue_difference_degrees, linear_rgb_to_oklch};
use crate::tone::agx::AgxBase;
use crate::tone::agx_settings::AgxSettings;
use crate::tone::working_io::WorkingInputGamut;

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

/// Стенд: основа filmic + матрицы sRGB для метрики.
pub(super) struct Rig {
    pub(super) base: Arc<FilmicBase>,
    pub(super) settings: FilmicSettings,
    srgb_to_xyz: Matrix3,
    srgb_to_working: PixelMatrix,
}

fn srgb_to_working() -> PixelMatrix {
    WORKING_PRIMARIES
        .rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&Primaries::SRGB.rgb_to_xyz().unwrap())
        .to_f32()
}

impl Rig {
    pub(super) fn new(settings: FilmicSettings) -> Self {
        Self {
            base: FilmicBase::resolve(&settings, DisplayGamut::Srgb).unwrap(),
            settings,
            srgb_to_xyz: Primaries::SRGB.rgb_to_xyz().unwrap(),
            srgb_to_working: srgb_to_working(),
        }
    }

    pub(super) fn with_norm(norm: FilmicNorm) -> Self {
        Self::new(FilmicSettings {
            norm,
            ..FilmicSettings::default()
        })
    }

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

fn max_hue_shift(path: &[Oklch]) -> f64 {
    let start = path[0].hue_degrees;
    path.iter()
        .filter(|point| point.chroma >= HUE_CHROMA_FLOOR)
        .map(|point| hue_difference_degrees(point.hue_degrees, start))
        .fold(0.0, f64::max)
}

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

const ORANGE: [f64; 3] = [214.0, 126.0, 44.0];
const SUNSET: [f64; 3] = [255.0, 120.0, 20.0];
const SKIN: [f64; 3] = [194.0, 150.0, 130.0];
const SKY: [f64; 3] = [98.0, 122.0, 157.0];

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

/// Наибольший сдвиг оттенка на колесе из 24 оттенков (умеренная насыщенность) на e = −1, 0, +1.
fn wheel_worst_shift(rig: &Rig) -> f64 {
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
    worst
}

/// Средняя цветность выхода по колесу из 24 оттенков на e = 0 (насыщенность 0,7).
fn wheel_mean_chroma(output_at: impl Fn(f64, [f64; 3]) -> Oklch) -> f64 {
    let mut sum = 0.0;
    for step in 0..24 {
        let [red, green, blue] = hsv_to_srgb8(f64::from(step) * 0.25, 0.7, 0.6);
        sum += output_at(0.0, unit_luminance(from_srgb8(red, green, blue))).chroma;
    }
    sum / 24.0
}

/// Эталон «чистый AgX»: умолчания AgX с ЯВНО выключенным возвратом оттенка (его умолчание будет переключено на 1).
fn agx_pure_output_at(unit_color: [f64; 3], e: f64) -> Oklch {
    let settings = AgxSettings {
        hue_restoration: 0.0,
        ..AgxSettings::default()
    };
    let base = AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap();
    let scale = f64::from(settings.scene_middle_gray) * e.exp2();
    let working = srgb_to_working().apply(unit_color.map(|channel| (channel * scale) as f32));
    let output = base.to_output(base.map_scene_pixel(working, 1.0));
    linear_rgb_to_oklch(&Primaries::SRGB.rgb_to_xyz().unwrap(), output)
}

const COLOUR_NORMS: [FilmicNorm; 3] = [FilmicNorm::MaxRgb, FilmicNorm::Luminance, FilmicNorm::PowerNorm];

fn unit_of(color: [f64; 3]) -> [f64; 3] {
    unit_luminance(from_srgb8(color[0], color[1], color[2]))
}

#[test]
fn decision_47_ceilings_hold_for_every_chroma_preserving_norm() {
    // Потолки решений 47 и 181 (от e = 0 до +4 EV, OKLCh, выход sRGB): оранжевый и кожа ≤ 4°, закат ≤ 5°,
    // небо ≤ 6°. Замер: оранжевый 0,2°, кожа 0,04°, небо 0,0°, закат 2,5° (AgX без возврата: 10,2°/1,2°/0,8°/5,5°;
    // AgX с возвратом: 3,9°/1,6°/3,2°/4,7°). Закат держится только с мягкими порогами сжатия выхода (0,95/1,25):
    // с порогами ACES RGC (0,80–0,88) он уходит на 6,5° — это сжатие, а не кривая.
    for norm in COLOUR_NORMS {
        let rig = Rig::with_norm(norm);
        for (name, color, ceiling) in [
            ("оранжевый", ORANGE, 4.0),
            ("кожа", SKIN, 4.0),
            ("закат", SUNSET, 5.0),
            ("небо", SKY, 6.0),
        ] {
            let shift = max_hue_shift(&rig.trajectory(unit_of(color), 4.0));
            assert!(
                shift <= ceiling,
                "{norm:?} {name}: сдвиг {shift}° выше потолка {ceiling}°"
            );
        }
    }
}

#[test]
fn decision_47_ceilings_break_with_the_aces_gamut_thresholds() {
    // Страховка смысла порогов по умолчанию: со старыми порогами ACES RGC закат выше потолка 5°. Если этот тест
    // упал — пороги или кривая изменились, и вывод в доке `DEFAULT_OUTPUT_COMPRESSION` надо пересмотреть.
    let rig = Rig::new(FilmicSettings {
        output_gamut_compression: crate::gamut::GamutCompressionSettings::default(),
        ..FilmicSettings::default()
    });
    let sunset = max_hue_shift(&rig.trajectory(unit_of(SUNSET), 4.0));
    assert!(sunset > 5.0, "закат {sunset}°");
}

#[test]
fn per_channel_norm_does_not_promise_the_ceilings() {
    // Без нормы кривая по каналам уводит оттенок (замер: оранжевый 51°, закат 65°) и красный к +6 EV ещё не белый.
    // Страховка от «тихого улучшения»/ухудшения: значения лежат около измеренных.
    let rig = Rig::with_norm(FilmicNorm::PerChannel);
    let orange = max_hue_shift(&rig.trajectory(unit_of(ORANGE), 4.0));
    let sunset = max_hue_shift(&rig.trajectory(unit_of(SUNSET), 4.0));
    assert!((40.0..60.0).contains(&orange), "оранжевый {orange}°");
    assert!((55.0..75.0).contains(&sunset), "закат {sunset}°");
    let red = rig.trajectory(unit_luminance(SIX[0].1), 6.0);
    assert!(
        red.last().unwrap().chroma > 0.02,
        "красный дошёл до белого: {:?}",
        red.last()
    );
}

#[test]
fn notorious_six_go_to_white_smoothly_and_monotonically() {
    // Пути R, G, B, C, M, Y от e = 0 до +6 EV: яркость не убывает, шаг яркости на 0,05 EV ограничен, скачок оттенка
    // мал, в конце белый. Замер на шаге яркости: MaxRgb/PowerNorm ≤ 0,0186, Luminance ≤ 0,0268 (чистый синий
    // при норме по яркости упирается в вписывание в охват и поднимается круче). Потолок AgX (0,02) держат две
    // нормы; для Luminance потолок 0,03 — замер с запасом 10 %, а не принятая цель.
    for (norm, step_ceiling) in [
        (FilmicNorm::MaxRgb, 0.02),
        (FilmicNorm::PowerNorm, 0.02),
        (FilmicNorm::Luminance, 0.03),
    ] {
        let rig = Rig::with_norm(norm);
        for (name, color) in SIX {
            let path = rig.trajectory(unit_luminance(color), 6.0);
            let steps = lightness_steps(&path);
            assert!(
                steps.iter().all(|step| *step >= -1e-6),
                "{norm:?} {name}: яркость убывает"
            );
            let max_step = steps.iter().copied().fold(0.0, f64::max);
            assert!(max_step < step_ceiling, "{norm:?} {name}: шаг яркости {max_step}");
            let hue_step = max_hue_step(&path);
            assert!(hue_step < 1.0, "{norm:?} {name}: скачок оттенка {hue_step}°");
            // Цветность не растёт на пути в белое после +3,5 EV.
            let from = (3.5 / EXPOSURE_STEP_EV).round() as usize;
            for pair in path[from..].windows(2) {
                assert!(
                    pair[1].chroma <= pair[0].chroma + 2e-4,
                    "{norm:?} {name}: цветность растёт"
                );
            }
            let last = path.last().unwrap();
            assert!(
                last.chroma < 0.02 && last.lightness > 0.985,
                "{norm:?} {name}: в конце не белый: {last:?}"
            );
        }
    }
}

#[test]
fn hue_wheel_keeps_hue_for_every_chroma_preserving_norm() {
    // 24 оттенка на e = −1, 0, +1. Замер: MaxRgb 0,4°, PowerNorm 0,2°, Luminance 0,0° (AgX без возврата: 6,9°).
    for norm in COLOUR_NORMS {
        let worst = wheel_worst_shift(&Rig::with_norm(norm));
        assert!(worst < 1.0, "{norm:?}: сдвиг на колесе {worst}°");
    }
    assert!(wheel_worst_shift(&Rig::with_norm(FilmicNorm::PerChannel)) > 5.0);
}

#[test]
fn mean_chroma_against_pure_agx_matches_the_measurement() {
    // Средняя цветность выхода по колесу из 24 оттенков (насыщенность 0,7, e = 0) к чистому AgX (0,1628).
    // Замер: PerChannel 1,09, Luminance 0,98, PowerNorm 0,81, MaxRgb 0,78 — норма по яркости цветность не
    // трогает, максимум и степенная норма её снижают (насыщенные цвета имеют норму выше яркости и раньше
    // входят в обесцвечивание светов).
    let agx = wheel_mean_chroma(|e, color| agx_pure_output_at(color, e));
    let ratio = |norm| {
        let rig = Rig::with_norm(norm);
        wheel_mean_chroma(|e, color| rig.output_at(color, e)) / agx
    };
    assert!((0.95..1.03).contains(&ratio(FilmicNorm::Luminance)));
    assert!((0.75..0.87).contains(&ratio(FilmicNorm::PowerNorm)));
    assert!((0.72..0.84).contains(&ratio(FilmicNorm::MaxRgb)));
    assert!((1.03..1.15).contains(&ratio(FilmicNorm::PerChannel)));
}

#[test]
fn clipped_sensor_highlights_go_to_white_for_max_and_power_norms() {
    // Реальный пересвет (решение 177): самый яркий канал упёрся в белый точки L, остальные ниже. Пиксель задан
    // прямо в W: `(L, k₁·L, k₂·L)`. Максимум — ровно белый; степенная норма — белый в пределах округления.
    // Норма по яркости такой пиксель белым НЕ делает (оранжевый остаётся оранжевым) — это её известная
    // слабость, поэтому не умолчание.
    let to_xyz = Primaries::SRGB.rgb_to_xyz().unwrap();
    for white in [1.0_f32, 1.6] {
        for (k1, k2) in [(0.6_f32, 0.3_f32), (0.3, 0.1), (0.9, 0.8), (0.05, 0.02)] {
            let pixel = [white, k1 * white, k2 * white];
            let max_base = FilmicBase::resolve(
                &FilmicSettings {
                    norm: FilmicNorm::MaxRgb,
                    ..FilmicSettings::default()
                },
                DisplayGamut::Srgb,
            )
            .unwrap();
            assert_eq!(
                max_base.map_scene_pixel(pixel, white),
                [1.0; 3],
                "MaxRgb L={white} {pixel:?}"
            );
            let power_base = FilmicBase::resolve(&FilmicSettings::default(), DisplayGamut::Srgb).unwrap();
            let output = power_base.to_output(power_base.map_scene_pixel(pixel, white));
            let lch = linear_rgb_to_oklch(&to_xyz, output);
            assert!(lch.lightness > 0.93, "PowerNorm L={white} {pixel:?}: {lch:?}");
            // Чем ближе второй канал к первому, тем белее; даже (1; 0,05; 0,02) обязан быть светлым и почти нейтральным.
            if k1 >= 0.3 {
                assert!(lch.chroma < 0.03, "PowerNorm L={white} {pixel:?}: {lch:?}");
            }
        }
    }
    let luminance = FilmicBase::resolve(
        &FilmicSettings {
            norm: FilmicNorm::Luminance,
            ..FilmicSettings::default()
        },
        DisplayGamut::Srgb,
    )
    .unwrap();
    let orange_wash = luminance.to_output(luminance.map_scene_pixel([1.0, 0.3, 0.1], 1.0));
    assert!(
        orange_wash[2] < 0.9,
        "норма по яркости выбелила пересвет: {orange_wash:?}"
    );
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

fn beyond_gamut_pass(norm: FilmicNorm, input_gamut: WorkingInputGamut) {
    let rig = Rig::new(FilmicSettings {
        norm,
        input_gamut,
        ..FilmicSettings::default()
    });
    let xyz_to_working = WORKING_PRIMARIES.rgb_to_xyz().unwrap().inverse().unwrap().to_f32();
    let white = (0.3127_f64, 0.3290_f64);
    for (name, x, y) in BEYOND_REC2020 {
        let mut previous_hue: Option<f64> = None;
        for step in 0..=100 {
            let t = f64::from(step) / 100.0;
            let (cx, cy) = (white.0 + (x - white.0) * t, white.1 + (y - white.1) * t);
            let luminance = f64::from(rig.settings.scene_middle_gray);
            let xyz = [cx / cy * luminance, luminance, (1.0 - cx - cy) / cy * luminance];
            let working = xyz_to_working.apply(xyz.map(|c| c as f32));
            let display = rig.base.map_scene_pixel(working, 1.0);
            let output = rig.base.to_output(display);
            assert!(
                display.iter().all(|c| c.is_finite() && *c >= 0.0),
                "{norm:?} {name} t={t}: {display:?}"
            );
            assert!(
                output.iter().all(|c| (0.0..=1.0).contains(c)),
                "{norm:?} {name} t={t}: {output:?}"
            );
            let lch = linear_rgb_to_oklch(&rig.srgb_to_xyz, output);
            if lch.chroma >= HUE_CHROMA_FLOOR {
                if let Some(previous) = previous_hue {
                    let jump = hue_difference_degrees(lch.hue_degrees, previous);
                    assert!(jump < 4.0, "{norm:?} {name} t={t}: скачок оттенка {jump}°");
                }
                previous_hue = Some(lch.hue_degrees);
            }
        }
        let mut previous_lightness: Option<f64> = None;
        for step in 0..=160 {
            let e = -4.0 + f64::from(step) * EXPOSURE_STEP_EV;
            let luminance = f64::from(rig.settings.scene_middle_gray) * e.exp2();
            let xyz = [x / y * luminance, luminance, (1.0 - x - y) / y * luminance];
            let working = xyz_to_working.apply(xyz.map(|c| c as f32));
            let output = rig.base.to_output(rig.base.map_scene_pixel(working, 1.0));
            let lightness = linear_rgb_to_oklch(&rig.srgb_to_xyz, output).lightness;
            if let Some(previous) = previous_lightness {
                // Без нормы (PerChannel) яркость OKLab на спектральных цветах не монотонна — обрезка каналов
                // меняет цветность; проверка яркости — для норм с сохранением цветности.
                if norm != FilmicNorm::PerChannel {
                    assert!(lightness - previous >= -1e-6, "{norm:?} {name} e={e}: яркость убывает");
                    assert!(lightness - previous < 0.03, "{norm:?} {name} e={e}: ступенька яркости");
                }
            }
            previous_lightness = Some(lightness);
        }
    }
}

#[test]
fn colors_beyond_rec2020_are_safe_with_clip_to_zero_input() {
    for norm in ALL_NORMS_FOR_GAMUT {
        beyond_gamut_pass(norm, WorkingInputGamut::ClipToZero);
    }
}

#[test]
fn colors_beyond_rec2020_are_safe_with_soft_compress_input() {
    for norm in ALL_NORMS_FOR_GAMUT {
        beyond_gamut_pass(
            norm,
            WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
        );
    }
}

const ALL_NORMS_FOR_GAMUT: [FilmicNorm; 4] = [
    FilmicNorm::PerChannel,
    FilmicNorm::MaxRgb,
    FilmicNorm::Luminance,
    FilmicNorm::PowerNorm,
];
