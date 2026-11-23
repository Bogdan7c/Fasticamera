//! Приёмка колёс цвета по зонам (решения 186, 191–193) через настоящую цепочку эталона.
//!
//! Серые патчи сцены проходят основу тона (AgX, ACES, filmic) → стадию цвета вида (колёса) → `to_output`, как в
//! `run_reference_working`. Проверяется то, что видит человек на экране: Lift красит только тени, Gain — только
//! света, Gamma — середину, серое остаётся серым, рампа гладкая, а в настоящем прогоне кадра колёса доходят до
//! итогового сигнала экрана. Таблицы печатаются для отчёта сессии.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "интеграционный тест: падение — способ сообщить о провале, таблица печатается для владельца"
)]

mod support;

use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::color::chromaticity::Primaries;
use isp_model::color::matrix::Matrix3;
use isp_model::color::oklab_working::OklabWorkspace;
use isp_model::color::working_space::WORKING_PRIMARIES;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::look::LookControls;
use isp_model::look::color::LookColorParams;
use isp_model::look::model::LookModelSettings;
use isp_model::metrics::{DisplayToLab, delta_e_2000};
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference_working;
use isp_model::tone::aces::{AcesBase, AcesSettings};
use isp_model::tone::agx::AgxBase;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::filmic::{FilmicBase, FilmicSettings};
use isp_model::tone::working_stage::WorkingToneBase;
use isp_model::transfer::srgb_decode;
use look_contract::{ColorWheel, ColorWheels, LookKnob, LookSettings, WheelPush, WheelZone};
use support::{assets_dir, real_capture::load_real_capture};

/// Серый сцены, который основы по умолчанию переводят в серый экрана 0,20 (одинаков у AgX, ACES и filmic).
const SCENE_MIDDLE_GRAY: f64 = 0.075;
/// Подтяжка AE на ±½ ступени: сцена умножается на `k`, и серый попадает на экран выше/ниже серого экрана.
const AE_NUDGES: [f64; 3] = [std::f64::consts::FRAC_1_SQRT_2, 1.0, std::f64::consts::SQRT_2];
/// Оттенки OKLCh: синий и оранжевый.
const BLUE_HUE: f32 = 265.0;
const ORANGE_HUE: f32 = 60.0;

// ---------- стенд ----------

/// Основа тона + всё, что нужно, чтобы прогнать серый патч сцены до вывода sRGB.
struct Rig {
    name: &'static str,
    base: Box<dyn WorkingToneBase>,
    srgb_to_working: Matrix3,
    to_lab: DisplayToLab,
    output_oklab: OklabWorkspace,
}

macro_rules! arc_base {
    ($wrapper:ident, $base:ty) => {
        struct $wrapper(Arc<$base>);
        impl WorkingToneBase for $wrapper {
            fn output_gamut(&self) -> DisplayGamut {
                self.0.output_gamut()
            }
            fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
                self.0.xyz_d50_to_working(xyz_d50)
            }
            fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
                self.0.map_scene_pixel(scene, white)
            }
            fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
                self.0.to_output(display_working)
            }
        }
    };
}
arc_base!(AgxShared, AgxBase);
arc_base!(AcesShared, AcesBase);
arc_base!(FilmicShared, FilmicBase);

fn rigs() -> Vec<Rig> {
    let gamut = DisplayGamut::Srgb;
    let bases: [(&'static str, Box<dyn WorkingToneBase>); 3] = [
        (
            "agx",
            Box::new(AgxShared(AgxBase::resolve(&AgxSettings::default(), gamut).unwrap())),
        ),
        (
            "aces",
            Box::new(AcesShared(AcesBase::resolve(&AcesSettings::default(), gamut).unwrap())),
        ),
        (
            "filmic",
            Box::new(FilmicShared(
                FilmicBase::resolve(&FilmicSettings::default(), gamut).unwrap(),
            )),
        ),
    ];
    let srgb_to_working = WORKING_PRIMARIES
        .rgb_to_xyz()
        .unwrap()
        .inverse()
        .unwrap()
        .mul(&Primaries::SRGB.rgb_to_xyz().unwrap());
    bases
        .into_iter()
        .map(|(name, base)| Rig {
            name,
            base,
            srgb_to_working,
            to_lab: DisplayToLab::new(gamut).unwrap(),
            output_oklab: OklabWorkspace::new(&Primaries::SRGB).unwrap(),
        })
        .collect()
}

impl Rig {
    /// Серый патч сцены `ev` ступеней от среднего серого, подтяжка AE `nudge` → линейный выход sRGB.
    /// `look` — стадия цвета вида (колёса) между основой и `to_output`; `None` — вида нет вовсе.
    fn render_grey(&self, ev: f64, nudge: f64, look: Option<&LookColorParams>) -> [f32; 3] {
        let level = SCENE_MIDDLE_GRAY * ev.exp2() * nudge;
        let scene = self.srgb_to_working.apply([level; 3]).map(|channel| channel as f32);
        let mut display = self.base.map_scene_pixel(scene, 1.0);
        if let Some(look) = look {
            display = look.apply(display);
        }
        self.base.to_output(display)
    }

    fn oklab(&self, output: [f32; 3]) -> [f32; 3] {
        self.output_oklab.to_lab(output)
    }

    fn delta_e(&self, first: [f32; 3], second: [f32; 3]) -> f64 {
        delta_e_2000(self.to_lab.convert(first), self.to_lab.convert(second))
    }
}

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap()
}

fn wheel(hue_degrees: f32, strength: f32, ring: f32) -> ColorWheel {
    ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, strength).unwrap(),
        ring: knob(ring),
    }
}

/// Стадия цвета вида с одним колесом (остальные нейтральны).
fn look_with(zone: WheelZone, value: ColorWheel) -> LookColorParams {
    look_from(ColorWheels::NEUTRAL.with_wheel(zone, value))
}

fn look_from(wheels: ColorWheels) -> LookColorParams {
    let mut settings = LookSettings::NEUTRAL;
    settings.wheels = wheels;
    LookColorParams::resolve(&settings, &LookModelSettings::default(), &WORKING_PRIMARIES)
        .unwrap()
        .params
        .expect("колёса включают стадию")
}

/// Сдвиг патча колесом: (ΔE2000, ΔOKLab a, ΔOKLab b, ΔOKLab L) относительно прогона без вида.
fn shift(rig: &Rig, ev: f64, nudge: f64, look: &LookColorParams) -> (f64, f32, f32, f32) {
    let plain = rig.render_grey(ev, nudge, None);
    let touched = rig.render_grey(ev, nudge, Some(look));
    let (before, after) = (rig.oklab(plain), rig.oklab(touched));
    (
        rig.delta_e(plain, touched),
        after[1] - before[1],
        after[2] - before[2],
        after[0] - before[0],
    )
}

/// Печатает строку таблицы: сдвиги патчей по EV.
fn print_row(label: &str, rig: &Rig, nudge: f64, look: &LookColorParams, patches: &[f64]) {
    let cells: Vec<String> = patches
        .iter()
        .map(|&ev| {
            let (delta_e, _, delta_b, delta_l) = shift(rig, ev, nudge, look);
            format!("{ev:+.1}EV: dE {delta_e:5.2} db {delta_b:+.3} dL {delta_l:+.3}")
        })
        .collect();
    eprintln!("{label:12} {:6} k={nudge:.2} | {}", rig.name, cells.join(" | "));
}

// ---------- зоны ----------

/// Патчи сцены, ступени от среднего серого.
const SHADOW_PATCHES: [f64; 2] = [-4.0, -3.0];
const HIGHLIGHT_PATCHES: [f64; 2] = [1.5, 2.5];
/// Патчи для «не тронуто»: тени, середина, света — каждый проверяется по правилу «по какую сторону серого экрана».
const ALL_PATCHES: [f64; 6] = [-6.0, -4.0, -3.0, 0.0, 1.5, 2.5];
/// Незаметная разница: ΔE2000 ниже 0,05 (порог видимости 1,0 — в 20 раз выше).
const UNTOUCHED_DELTA_E: f64 = 0.05;

/// Ступени патча на экране относительно серого экрана: подтяжка AE `k` сдвигает патч на `log2(k)` ступеней.
/// Серый экрана (пивот колёс) лежит на 0 такой шкалы: сцена `0 EV` при `k = 1` попадает точно в него.
fn effective_ev(ev: f64, nudge: f64) -> f64 {
    ev + nudge.log2()
}

#[test]
fn lift_toward_blue_tints_only_the_shadows_below_display_grey() {
    let mut tinted_patches = 0;
    for rig in rigs() {
        let lift = look_with(WheelZone::Lift, wheel(BLUE_HUE, 1.0, 0.0));
        for nudge in AE_NUDGES {
            print_row("lift blue", &rig, nudge, &lift, &ALL_PATCHES);
            for ev in ALL_PATCHES {
                let (delta_e, _, delta_b, _) = shift(&rig, ev, nudge, &lift);
                let label = format!("{} k={nudge:.2} {ev:+}EV", rig.name);
                if effective_ev(ev, nudge) >= 0.0 {
                    // На сером экрана и светлее Lift ровно ноль: ни цвета, ни света.
                    assert!(delta_e < UNTOUCHED_DELTA_E, "{label}: Lift тронул ΔE {delta_e}");
                } else if SHADOW_PATCHES.contains(&ev) {
                    // Глубокие тени (на экране −2,5…−4,5 ступени от серого) заметно синеют.
                    assert!(delta_e > 2.0 && delta_b < -0.02, "{label}: ΔE {delta_e}, Δb {delta_b}");
                    tinted_patches += 1;
                }
            }
        }
    }
    assert_eq!(
        tinted_patches,
        3 * 3 * 2,
        "каждая основа и подтяжка проверили оба теневых патча"
    );
}

#[test]
fn gain_toward_orange_warms_only_the_highlights_above_display_grey() {
    let mut warmed_patches = 0;
    for rig in rigs() {
        let gain = look_with(WheelZone::Gain, wheel(ORANGE_HUE, 1.0, 0.0));
        for nudge in AE_NUDGES {
            print_row("gain orange", &rig, nudge, &gain, &ALL_PATCHES);
            for ev in ALL_PATCHES {
                let (delta_e, delta_a, delta_b, _) = shift(&rig, ev, nudge, &gain);
                let label = format!("{} k={nudge:.2} {ev:+}EV", rig.name);
                if effective_ev(ev, nudge) <= 0.0 {
                    assert!(delta_e < UNTOUCHED_DELTA_E, "{label}: Gain тронул ΔE {delta_e}");
                } else if HIGHLIGHT_PATCHES.contains(&ev) {
                    // Оранжевый = +a и +b: света теплеют.
                    assert!(
                        delta_e > 3.0 && delta_b > 0.01 && delta_a > 0.0,
                        "{label}: ΔE {delta_e}, Δb {delta_b}"
                    );
                    warmed_patches += 1;
                }
            }
        }
    }
    assert_eq!(warmed_patches, 3 * 3 * 2);
}

#[test]
fn gamma_moves_the_middle_and_barely_touches_deep_shadow_and_near_white() {
    const DEEP_SHADOW_EV: f64 = -6.0;
    const NEAR_WHITE_EV: f64 = 4.0;
    for rig in rigs() {
        let gamma = look_with(WheelZone::Gamma, wheel(BLUE_HUE, 1.0, 0.0));
        for nudge in AE_NUDGES {
            // Середина — тот патч, что лёг ближе всего к серому экрана при этой подтяжке.
            let middle_ev = -nudge.log2();
            let middle = shift(&rig, middle_ev, nudge, &gamma).0;
            let deep_shadow = shift(&rig, DEEP_SHADOW_EV, nudge, &gamma).0;
            let near_white = shift(&rig, NEAR_WHITE_EV, nudge, &gamma).0;
            eprintln!(
                "gamma blue   {:6} k={nudge:.2} | середина dE {middle:5.2} | -6EV dE {deep_shadow:5.2} | +4EV dE {near_white:5.2}",
                rig.name
            );
            assert!(
                middle > 8.0,
                "{} k={nudge:.2}: середина почти не сдвинулась, ΔE {middle}",
                rig.name
            );
            assert!(deep_shadow < 0.2 * middle && near_white < 0.2 * middle);
        }
    }
}

#[test]
fn ring_of_each_wheel_changes_lightness_in_its_own_zone_only() {
    for rig in rigs() {
        let lift_ring = look_with(WheelZone::Lift, wheel(0.0, 0.0, 1.0));
        let gain_ring = look_with(WheelZone::Gain, wheel(0.0, 0.0, 1.0));
        // Lift +1: тень светлее, свет не тронут. Gain +1: наоборот (патчи без обрезки: −3 и +1,5 ступени).
        let (_, _, _, shadow_lightness) = shift(&rig, -3.0, 1.0, &lift_ring);
        let (delta_e_high, ..) = shift(&rig, 1.5, 1.0, &lift_ring);
        let (_, _, _, highlight_lightness) = shift(&rig, 1.5, 1.0, &gain_ring);
        let (delta_e_low, ..) = shift(&rig, -3.0, 1.0, &gain_ring);
        eprintln!(
            "кольца       {:6} | Lift+1: dL(-3EV) {shadow_lightness:+.3}, dE(+1.5EV) {delta_e_high:.2} | Gain+1: dL(+1.5EV) {highlight_lightness:+.3}, dE(-3EV) {delta_e_low:.2}",
            rig.name
        );
        assert!(
            shadow_lightness > 0.02,
            "{}: Lift +1 не поднял тень: {shadow_lightness}",
            rig.name
        );
        assert!(
            highlight_lightness > 0.02,
            "{}: Gain +1 не поднял свет: {highlight_lightness}",
            rig.name
        );
        assert!(delta_e_high < UNTOUCHED_DELTA_E && delta_e_low < UNTOUCHED_DELTA_E);
    }
}

// ---------- серое, гладкость, нейтраль ----------

/// Во сколько раз колесо может усилить наибольший изгиб рампы L и цветности (вторая разность на шаге 0,01 EV).
/// Smoothstep без изломов добавляет изгиб порядка `C_max·w''·(шаг по L)²` — доли основного изгиба основы; 1,5 — запас.
const BEND_LIMIT_FACTOR: f32 = 1.5;
/// Пол изгиба: у серого без колёс цветность ≈ 0 и её изгиб ≈ 0, относительный порог там бессмыслен; 1e-5 — уровень,
/// ниже которого вторая разность невидима (в сотни раз меньше шага 8 бит, 1/255 ≈ 4e-3).
/// Запас от краёв охвата выхода (линейный свет): ближе к чёрному и белому выход обрезается. Чёрный край уже: кривая
/// основы у тёмного края своя, и тонкая тень (сотые доли процента) ещё различима.
const BLACK_EDGE_MARGIN: f32 = 1e-4;
const WHITE_EDGE_MARGIN: f32 = 1e-3;
const BEND_FLOOR: f32 = 1e-5;

/// Рампа серого сцены: `from..=to` ступеней, шаг `step`.
fn ev_ramp(from: f64, to: f64, step: f64) -> Vec<f64> {
    let count = ((to - from) / step).round() as usize;
    (0..=count).map(|index| from + step * index as f64).collect()
}

/// Цветность OKLab выхода.
fn chroma(rig: &Rig, output: [f32; 3]) -> f32 {
    let [_, a, b] = rig.oklab(output);
    a.hypot(b)
}

/// Все колёса тронуты кольцами (без цвета) на полную силу в разные стороны.
fn rings_only_wheels() -> ColorWheels {
    ColorWheels {
        lift: wheel(0.0, 0.0, 1.0),
        gamma: wheel(0.0, 0.0, -1.0),
        gain: wheel(0.0, 0.0, 1.0),
        offset: wheel(0.0, 0.0, -0.5),
    }
}

#[test]
fn grey_stays_grey_with_neutral_wheels_and_with_rings_only() {
    // Порог цветности 1e-3 OKLab (~0,2 ΔE): на порядок ниже заметного, выше шума f32 матриц.
    const GREY_CHROMA_LIMIT: f32 = 1e-3;
    let rings = look_from(rings_only_wheels());
    for rig in rigs() {
        let mut peak_with_rings = 0.0_f32;
        let mut peak_without = 0.0_f32;
        for nudge in AE_NUDGES {
            for ev in ev_ramp(-8.0, 4.0, 0.25) {
                peak_without = peak_without.max(chroma(&rig, rig.render_grey(ev, nudge, None)));
                let with_rings = chroma(&rig, rig.render_grey(ev, nudge, Some(&rings)));
                assert!(
                    with_rings < GREY_CHROMA_LIMIT,
                    "{} k={nudge:.2} {ev:+}EV: цветность {with_rings}",
                    rig.name
                );
                peak_with_rings = peak_with_rings.max(with_rings);
            }
        }
        eprintln!(
            "серое        {:6} | пик цветности: без колёс {peak_without:.1e}, кольца {peak_with_rings:.1e}",
            rig.name
        );
        assert!(peak_without < GREY_CHROMA_LIMIT);
    }
}

/// Наибольшая вторая разность ряда `values` по тройкам соседних точек рампы, у которых ни один канал выхода не
/// прижат к краю охвата (`BLACK_EDGE_MARGIN` от 0 или `WHITE_EDGE_MARGIN` от 1): там выход обрезается, и излом даёт обрезка (она есть и без
/// колёс — у голого серого у чёрного и у белого), а не колесо. Возвращает ещё число учтённых троек.
/// Тройка считается и по выходам без колёс: излом основы на краю чёрного (в рампе без колёс его скрывает обрезка)
/// колесо только «вытаскивает» на свет, а не создаёт.
fn max_bend_inside_gamut(neutral_outputs: &[[f32; 3]], outputs: &[[f32; 3]], values: &[f32]) -> (f32, usize) {
    let inside = |output: &[f32; 3]| {
        output
            .iter()
            .all(|&channel| channel > BLACK_EDGE_MARGIN && channel < 1.0 - WHITE_EDGE_MARGIN)
    };
    let mut worst = 0.0_f32;
    let mut counted = 0;
    for ((triple, outs), neutral_outs) in values
        .windows(3)
        .zip(outputs.windows(3))
        .zip(neutral_outputs.windows(3))
    {
        if outs.iter().chain(neutral_outs).all(inside) {
            worst = worst.max((triple[2] - 2.0 * triple[1] + triple[0]).abs());
            counted += 1;
        }
    }
    (worst, counted)
}

#[test]
fn every_wheel_at_full_strength_keeps_the_grey_ramp_smooth_and_rings_keep_it_monotonic() {
    let ramp = ev_ramp(-10.0, 6.0, 0.01);
    for rig in rigs() {
        // Ряд: выходы, светлота OKLab, цветность OKLab.
        let series = |look: Option<&LookColorParams>| -> (Vec<[f32; 3]>, Vec<f32>, Vec<f32>) {
            let outputs: Vec<[f32; 3]> = ramp.iter().map(|&ev| rig.render_grey(ev, 1.0, look)).collect();
            let lightness = outputs.iter().map(|&output| rig.oklab(output)[0]).collect();
            let chroma_values = outputs.iter().map(|&output| chroma(&rig, output)).collect();
            (outputs, lightness, chroma_values)
        };
        let (neutral_outputs, neutral_lightness, neutral_chroma) = series(None);
        let (neutral_lightness_bend, counted) =
            max_bend_inside_gamut(&neutral_outputs, &neutral_outputs, &neutral_lightness);
        let (neutral_chroma_bend, _) = max_bend_inside_gamut(&neutral_outputs, &neutral_outputs, &neutral_chroma);
        eprintln!(
            "гладкость    {:6} | без колёс (учтено {counted} из {} точек): изгиб L {neutral_lightness_bend:.2e}, C {neutral_chroma_bend:.2e}",
            rig.name,
            ramp.len() - 2
        );
        assert!(counted > 600, "почти вся рампа вне охвата: учтено {counted}");
        for zone in WheelZone::ALL {
            for (kind, value) in [("цвет", wheel(BLUE_HUE, 1.0, 0.0)), ("кольцо", wheel(0.0, 0.0, 1.0))] {
                let (outputs, lightness, chroma_series) = series(Some(&look_with(zone, value)));
                let (lightness_bend, counted) = max_bend_inside_gamut(&neutral_outputs, &outputs, &lightness);
                let (chroma_bend, _) = max_bend_inside_gamut(&neutral_outputs, &outputs, &chroma_series);
                eprintln!(
                    "             {zone:?} {kind:7} | учтено {counted}: изгиб L {lightness_bend:.2e}, изгиб C {chroma_bend:.2e}"
                );
                assert!(
                    counted > 400,
                    "{} {zone:?} {kind}: учтено мало точек ({counted})",
                    rig.name
                );
                assert!(
                    lightness_bend <= BEND_LIMIT_FACTOR * neutral_lightness_bend + BEND_FLOOR,
                    "{} {zone:?} {kind}: L изогнулась на {lightness_bend}",
                    rig.name
                );
                assert!(
                    chroma_bend <= BEND_LIMIT_FACTOR * neutral_chroma_bend + BEND_FLOOR,
                    "{} {zone:?} {kind}: цветность изогнулась на {chroma_bend}",
                    rig.name
                );
            }
        }
        // Кольца ±1 по очереди и все вместе: светлота выхода не убывает вдоль рампы (допуск — шум f32).
        for sign in [-1.0, 1.0] {
            let all_rings = ColorWheels {
                lift: wheel(0.0, 0.0, sign),
                gamma: wheel(0.0, 0.0, sign),
                gain: wheel(0.0, 0.0, sign),
                offset: wheel(0.0, 0.0, sign),
            };
            let mut looks: Vec<LookColorParams> = WheelZone::ALL
                .iter()
                .map(|&zone| look_with(zone, wheel(0.0, 0.0, sign)))
                .collect();
            looks.push(look_from(all_rings));
            for look in &looks {
                let (_, lightness, _) = series(Some(look));
                let worst_drop = lightness
                    .windows(2)
                    .map(|pair| pair[0] - pair[1])
                    .fold(f32::MIN, f32::max);
                assert!(
                    worst_drop <= 1e-6,
                    "{} кольца {sign:+}: светлота убывает на {worst_drop}",
                    rig.name
                );
            }
        }
    }
}

#[test]
fn neutral_wheels_give_exactly_the_picture_without_any_look() {
    let mut settings = LookSettings::NEUTRAL;
    settings.wheels = ColorWheels::NEUTRAL;
    let resolved = LookColorParams::resolve(&settings, &LookModelSettings::default(), &WORKING_PRIMARIES).unwrap();
    assert!(
        resolved.params.is_none(),
        "нейтральные колёса не должны включать стадию цвета"
    );
    for rig in rigs() {
        for nudge in AE_NUDGES {
            for ev in ev_ramp(-8.0, 6.0, 0.5) {
                let plain = rig.render_grey(ev, nudge, None);
                let through_stage = rig.render_grey(ev, nudge, resolved.params.as_ref());
                assert_eq!(plain, through_stage, "{} k={nudge:.2} {ev:+}EV", rig.name);
            }
        }
    }
}

// ---------- настоящий прогон кадра ----------

const FRAME_SIDE: u32 = 64;
const FRAME_BLACK: f64 = 64.0;
const FRAME_WHITE: f64 = 1023.0;
/// Яркость левой (тёмной) и правой (светлой) половины кадра в нормированном свете после баланса белого.
const DARK_LEVEL: f64 = 0.012;
const BRIGHT_LEVEL: f64 = 0.5;
/// Нейтраль лампы из `pipeline_end_to_end` (R, G, B).
const LAMP: [f64; 3] = [0.515625, 1.0, 0.4990234375];

/// Кадр: левая половина — тёмный серый, правая — светлый серый (нейтраль лампы).
fn two_tone_raw() -> RawMosaic {
    let pattern = BayerPattern::BGGR;
    let samples = (0..FRAME_SIDE * FRAME_SIDE)
        .map(|index| {
            let (x, y) = (index % FRAME_SIDE, index / FRAME_SIDE);
            let level = if x < FRAME_SIDE / 2 { DARK_LEVEL } else { BRIGHT_LEVEL };
            let channel = pattern.channel_at(x, y).index();
            (FRAME_BLACK + level * LAMP[channel] * (FRAME_WHITE - FRAME_BLACK)).round() as u16
        })
        .collect();
    RawMosaic::new(PixelSize::new(FRAME_SIDE, FRAME_SIDE), pattern, samples).unwrap()
}

/// Прогоняет кадр через `run_reference_working` с видом `settings`; возвращает итоговый сигнал экрана.
fn run_frame(settings: LookSettings) -> isp_model::image::DisplayEncodedImage {
    let capture = load_real_capture(&assets_dir(), "real-main-lamp-gray-tile");
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    let controls = ProcessingControls::with_defaults(
        isp_model::color::dng::CameraNeutral::new(LAMP).unwrap(),
        DemosaicMethod::Quality,
        DisplayGamut::Srgb,
    )
    .with_look(LookControls::new(settings, Arc::new(LookModelSettings::default())));
    let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
    run_reference_working(&two_tone_raw(), &params).unwrap().display_encoded
}

/// Среднее OKLab `b` и `L` по столбцам `from..to` итогового сигнала (кодированный sRGB → линейный → OKLab).
fn mean_lab(image: &isp_model::image::DisplayEncodedImage, from: u32, to: u32) -> [f32; 3] {
    let workspace = OklabWorkspace::new(&Primaries::SRGB).unwrap();
    let mut total = [0.0_f32; 3];
    let mut count = 0.0;
    for y in 8..FRAME_SIDE - 8 {
        for x in from..to {
            let lab = workspace.to_lab(image.pixel(x, y).map(srgb_decode));
            total.iter_mut().zip(lab).for_each(|(sum, value)| *sum += value);
            count += 1.0;
        }
    }
    total.map(|sum| sum / count)
}

#[test]
fn lift_toward_blue_reaches_the_final_screen_signal_of_the_reference_pipeline() {
    let neutral = run_frame(LookSettings::NEUTRAL);
    let mut settings = LookSettings::NEUTRAL;
    settings.wheels = ColorWheels::NEUTRAL.with_wheel(WheelZone::Lift, wheel(BLUE_HUE, 1.0, 0.0));
    let blue_shadows = run_frame(settings);
    // Колонки вдали от шва между половинами (демозаик размазывает несколько пикселей).
    let (dark, bright) = ((6, 26), (38, 58));
    let dark_before = mean_lab(&neutral, dark.0, dark.1);
    let dark_after = mean_lab(&blue_shadows, dark.0, dark.1);
    let bright_before = mean_lab(&neutral, bright.0, bright.1);
    let bright_after = mean_lab(&blue_shadows, bright.0, bright.1);
    eprintln!(
        "кадр         | тень: b {:+.4} -> {:+.4}, L {:.3} -> {:.3} | свет: b {:+.4} -> {:+.4}, L {:.3} -> {:.3}",
        dark_before[2],
        dark_after[2],
        dark_before[0],
        dark_after[0],
        bright_before[2],
        bright_after[2],
        bright_before[0],
        bright_after[0]
    );
    assert!(
        dark_after[2] < dark_before[2] - 0.02,
        "тень не посинела: {dark_before:?} -> {dark_after:?}"
    );
    for channel in 0..3 {
        assert!(
            (bright_after[channel] - bright_before[channel]).abs() < 1e-3,
            "свет изменился в канале {channel}: {bright_before:?} -> {bright_after:?}"
        );
    }
}
