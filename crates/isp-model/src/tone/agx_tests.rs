//! Основа AgX: каждый шаг формулы на синтетике; сломанная формула роняет тест.

use std::sync::Arc;

use super::{AgxBase, AgxParameters, MIN_WHITE_LEVEL};
use crate::exposure::ExposureGain;
use crate::image::DisplayGamut;
use crate::tone::agx_settings::{AgxSettings, InvalidAgx};
use crate::tone::working_io::WorkingInputGamut;

fn default_base() -> Arc<AgxBase> {
    AgxBase::resolve(&AgxSettings::default(), DisplayGamut::Srgb).unwrap()
}

fn grey_pixel(value: f32) -> [f32; 3] {
    [value; 3]
}

/// Кривая из доки модуля в её ПЕРВОНАЧАЛЬНОМ виде (через `k1`, `E(u, p)`) в f64 — независимая от кода
/// форма без `k1`, по которой считает основа.
fn reference_curve(settings: &AgxSettings, e: f64, white_level: f64) -> f64 {
    let (s, gamma) = (f64::from(settings.slope), f64::from(settings.display_gamma));
    let y_p = f64::from(settings.display_middle_gray).powf(1.0 / gamma);
    let delta_sh = (white_level / f64::from(settings.scene_middle_gray)).log2();
    let delta_toe = f64::from(settings.toe_depth_ev);
    let basis = |u: f64, p: f64| u / (1.0 + u.powf(p)).powf(1.0 / p);
    if e >= delta_sh {
        return 1.0;
    }
    if e >= 0.0 {
        let p1 = f64::from(settings.shoulder_power);
        let r1 = s * delta_sh / (1.0 - y_p);
        let k1 = s * delta_sh / (r1.powf(p1) - 1.0).powf(1.0 / p1);
        y_p + k1 * basis(s * e / k1, p1)
    } else {
        let p0 = f64::from(settings.toe_power);
        let depth = -e;
        if depth >= delta_toe {
            return 0.0;
        }
        let r0 = s * delta_toe / y_p;
        let k0 = s * delta_toe / (r0.powf(p0) - 1.0).powf(1.0 / p0);
        y_p - k0 * basis(s * depth / k0, p0)
    }
}

#[test]
fn curve_matches_the_k1_form_derived_from_boundary_conditions() {
    let settings = AgxSettings::default();
    let base = default_base();
    let grey = settings.scene_middle_gray;
    for white in [1.0_f32, 2.5, 6.0] {
        for step in -60..=45 {
            let e = f64::from(step) / 10.0;
            let light = grey * (e as f32).exp2();
            let ours = f64::from(base.curve_channel(light, white));
            let expected = reference_curve(&settings, e, f64::from(white));
            assert!(
                (ours - expected).abs() < 3e-5,
                "белый {white}, e = {e}: {ours} против {expected}"
            );
        }
    }
}

#[test]
fn shoulder_constant_k1_reaches_exactly_one_at_the_point_white() {
    // Вывод k1 из граничного условия: y(Δ_sh) = y_p + k1·E(s·Δ_sh/k1) обязан равняться 1.
    let settings = AgxSettings::default();
    let (s, gamma) = (f64::from(settings.slope), f64::from(settings.display_gamma));
    let y_p = f64::from(settings.display_middle_gray).powf(1.0 / gamma);
    let p1 = f64::from(settings.shoulder_power);
    for white in [1.0_f64, 3.0, 8.0] {
        let delta = (white / f64::from(settings.scene_middle_gray)).log2();
        let r1 = s * delta / (1.0 - y_p);
        assert!(r1 > 1.0);
        let k1 = s * delta / (r1.powf(p1) - 1.0).powf(1.0 / p1);
        let u = s * delta / k1;
        let y = y_p + k1 * u / (1.0 + u.powf(p1)).powf(1.0 / p1);
        assert!((y - 1.0).abs() < 1e-12, "белый {white}: y(Δ_sh) = {y}");
    }
}

#[test]
fn middle_gray_lands_on_the_configured_display_gray() {
    let settings = AgxSettings::default();
    let base = default_base();
    let out = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray), 1.0);
    for channel in out {
        assert!(
            (channel - settings.display_middle_gray).abs() < 2e-6,
            "серый сцены → {channel}, ожидали {}",
            settings.display_middle_gray
        );
    }
    // До γ — опорный выход y_p.
    let y = base.curve_channel(settings.scene_middle_gray, 1.0);
    let expected = settings.display_middle_gray.powf(1.0 / settings.display_gamma);
    assert!((y - expected).abs() < 1e-6);
}

#[test]
fn grey_stays_grey_over_the_whole_range() {
    let base = default_base();
    for step in -120..=60 {
        let value = 0.075 * (step as f32 / 10.0).exp2();
        for white in [1.0, 4.0] {
            let working = base.map_scene_pixel(grey_pixel(value), white);
            let spread = working[0].max(working[1]).max(working[2]) - working[0].min(working[1]).min(working[2]);
            assert!(
                spread < 3e-6,
                "рабочее пространство: {value}, белый {white}: {working:?}"
            );
            let output = base.to_output(working);
            let spread = output[0].max(output[1]).max(output[2]) - output[0].min(output[1]).min(output[2]);
            assert!(spread < 3e-6, "выход: {value}, белый {white}: {output:?}");
        }
    }
}

#[test]
fn curve_is_c1_at_the_middle_gray() {
    let settings = AgxSettings::default();
    let base = default_base();
    let grey = f64::from(settings.scene_middle_gray);
    let y = |e: f64| f64::from(base.curve_channel((grey * e.exp2()) as f32, 1.0));
    let step = 1e-3;
    let left = (y(0.0) - y(-step)) / step;
    let right = (y(step) - y(0.0)) / step;
    // Наклон на сером равен s с обеих сторон (шаг 1e-3 ступени, f32 — допуск 5e-3).
    assert!((left - f64::from(settings.slope)).abs() < 5e-3, "слева {left}");
    assert!((right - f64::from(settings.slope)).abs() < 5e-3, "справа {right}");
    assert!((left - right).abs() < 5e-3, "излом: {left} против {right}");
}

#[test]
fn point_white_maps_to_exactly_one_and_black_depth_to_exactly_zero() {
    let settings = AgxSettings::default();
    let base = default_base();
    for white in [1.0_f32, 2.0, 5.5] {
        assert_eq!(base.curve_channel(white, white), 1.0, "x = L → ровно 1");
        assert_eq!(base.curve_channel(white * 3.0, white), 1.0, "выше L → ровно 1");
        assert!(base.curve_channel(white * 0.9, white) < 1.0);
    }
    let black_light = settings.scene_middle_gray * (-settings.toe_depth_ev).exp2();
    assert_eq!(base.curve_channel(black_light, 1.0), 0.0, "e = −Δ_toe → ровно 0");
    assert_eq!(base.curve_channel(black_light * 0.25, 1.0), 0.0);
    assert!(base.curve_channel(black_light * 2.0, 1.0) > 0.0);
}

#[test]
fn curve_is_monotone_and_higher_white_never_brightens() {
    let base = default_base();
    for white in [1.0_f32, 2.0, 6.0] {
        let mut previous = 0.0;
        for step in 1..=600 {
            let value = base.curve_channel(step as f32 * 0.01, white);
            assert!(
                value >= previous && value <= 1.0,
                "белый {white}, x = {}",
                step as f32 * 0.01
            );
            previous = value;
        }
    }
    // Белый точки дальше — плечо длиннее, тот же свет темнее (виньетированный угол не «выгорает»).
    for step in 1..=100 {
        let value = step as f32 * 0.01;
        assert!(base.curve_channel(value, 6.0) <= base.curve_channel(value, 1.0) + 1e-6);
    }
}

#[test]
fn nan_is_black_and_negative_or_far_out_of_gamut_never_produces_nan() {
    let base = default_base();
    assert_eq!(base.map_scene_pixel([f32::NAN, 0.5, 0.5], 1.0), [0.0; 3]);
    assert_eq!(base.map_scene_pixel([0.2, f32::NAN, 0.0], 1.0), [0.0; 3]);
    for pixel in [
        [-0.3, 0.8, 0.1],
        [-5.0, -5.0, -5.0],
        [1e-30, 0.0, -1e-30],
        [1e6, 0.0, 0.0],
        [f32::INFINITY, 0.0, 0.0],
    ] {
        let working = base.map_scene_pixel(pixel, 1.0);
        assert!(working.iter().all(|c| c.is_finite()), "{pixel:?} → {working:?}");
        let output = base.to_output(working);
        assert!(output.iter().all(|c| (0.0..=1.0).contains(c)), "{pixel:?} → {output:?}");
    }
}

fn multiply(left: [[f32; 3]; 3], right: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut product = [[0.0; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            product[row][column] = (0..3).map(|k| left[row][k] * right[k][column]).sum();
        }
    }
    product
}

#[test]
fn inset_rows_sum_to_one_and_equal_parameters_make_outset_the_exact_inverse() {
    let same = AgxSettings {
        outset: AgxSettings::default().inset,
        ..AgxSettings::default()
    };
    let parameters = AgxBase::resolve(&same, DisplayGamut::Srgb).unwrap().parameters();
    for row in parameters.inset {
        assert!((row.iter().sum::<f32>() - 1.0).abs() < 2e-7, "{row:?}");
    }
    let identity = multiply(parameters.outset, parameters.inset);
    for (row, expected_row) in identity.iter().zip([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]) {
        for (value, expected) in row.iter().zip(expected_row) {
            assert!((value - expected).abs() < 2e-6, "{identity:?}");
        }
    }
    // А с разными параметрами матрицы НЕ взаимно обратны (иначе outset ничего бы не значил).
    let default = default_base().parameters();
    let product = multiply(default.outset, default.inset);
    assert!(
        (product[0][0] - 1.0).abs() > 1e-2 || (product[0][1]).abs() > 1e-2,
        "{product:?}"
    );
}

#[test]
fn inset_pulls_a_pure_working_primary_toward_grey() {
    let settings = AgxSettings {
        inset: [crate::color::working_space::PrimaryAdjustment {
            amount: 0.3,
            rotation_degrees: 0.0,
        }; 3],
        ..AgxSettings::default()
    };
    let parameters = AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap().parameters();
    // Чистый красный Rec. 2020 (1, 0, 0) после «внутрь»: красный меньше 1, остальные > 0, сумма сохранена (белый не сдвинут).
    let red = [parameters.inset[0][0], parameters.inset[1][0], parameters.inset[2][0]];
    assert!(red[0] < 0.95 && red[0] > 0.6, "{red:?}");
    assert!(red[1] > 0.02 && red[2] > 0.0, "{red:?}");
}

/// Формула кривой, переписанная ТОЛЬКО по числам снимка для GPU — как её сделает шейдер.
fn shader_channel(p: &AgxParameters, x: f32, white: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    let e = (x / p.grey).log2();
    let delta = (white.max(MIN_WHITE_LEVEL) / p.grey).log2();
    if e >= delta {
        return 1.0;
    }
    let y = if e >= 0.0 {
        let ratio = p.shoulder_scale * delta;
        let excess = ratio.powf(p.shoulder_power) - 1.0;
        let blend = (e / delta).powf(p.shoulder_power) * excess;
        p.gray_output + p.slope * e / (1.0 + blend).powf(p.shoulder_inverse_power)
    } else {
        let depth = -e;
        if depth >= p.toe_depth {
            return 0.0;
        }
        let blend = (depth / p.toe_depth).powf(p.toe_power) * p.toe_excess;
        p.gray_output - p.slope * depth / (1.0 + blend).powf(p.toe_inverse_power)
    };
    y.clamp(0.0, 1.0)
}

fn shader_matrix(rows: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    rows.map(|r| r[0] * v[0] + r[1] * v[1] + r[2] * v[2])
}

#[test]
fn gpu_snapshot_reproduces_the_formula_bit_for_bit() {
    // Снимок формулы здесь — кривая и матрицы; шаг возврата оттенка (степень умолчания 1) переписан отдельно
    // и сверен с GPU в `isp-vulkan` (паритет степеней 0, 0,5, 1), поэтому для переписанной формулы он выключен.
    let settings = AgxSettings {
        hue_restoration: 0.0,
        ..AgxSettings::default()
    };
    let base = AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap();
    let p = base.parameters();
    assert!(p.input_compression.is_none(), "умолчание — обрезка нулём");
    let mut checked = 0;
    for white in [1.0_f32, 1.7, 6.0] {
        for r in [-0.05_f32, 0.0, 0.001, 0.02, 0.075, 0.3, 1.0, 4.0] {
            for g in [0.0_f32, 0.004, 0.075, 0.9] {
                for b in [0.0_f32, 0.05, 0.6, 2.0] {
                    let scene = [r, g, b];
                    let entering = scene.map(|c| c.max(0.0));
                    let inset = shader_matrix(&p.inset, entering);
                    let shaped = inset.map(|c| shader_channel(&p, c, white).powf(p.gamma));
                    let expected = shader_matrix(&p.outset, shaped);
                    let actual = base.map_scene_pixel(scene, white);
                    for channel in 0..3 {
                        assert_eq!(
                            actual[channel].to_bits(),
                            expected[channel].to_bits(),
                            "{scene:?}, белый {white}: {actual:?} против {expected:?}"
                        );
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 3 * 8 * 4 * 4);
}

#[test]
fn soft_compress_input_exports_its_own_parameters() {
    let settings = AgxSettings {
        input_gamut: WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
        ..AgxSettings::default()
    };
    let parameters = AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap().parameters();
    let exported = parameters.input_compression.expect("сжатие входа выбрано");
    assert_eq!(exported.threshold, [0.95; 3]);
    assert_ne!(exported, parameters.output_compression, "у входа и выхода свои пороги");
}

#[test]
fn exposure_gain_moves_the_grey_but_leaves_the_point_white_white() {
    let settings = AgxSettings::default();
    let lifted = settings.with_exposure_gain(ExposureGain::new(2.0).unwrap());
    let base = AgxBase::resolve(&lifted, DisplayGamut::Srgb).unwrap();
    // Серый сцены 0,075 при k = 2 светлее: серым становится свет 0,0375.
    let at_new_grey = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray / 2.0), 1.0);
    assert!(
        (at_new_grey[0] - settings.display_middle_gray).abs() < 2e-6,
        "{at_new_grey:?}"
    );
    let at_old_grey = base.map_scene_pixel(grey_pixel(settings.scene_middle_gray), 1.0);
    assert!(
        at_old_grey[0] > settings.display_middle_gray + 0.05,
        "светлее прежнего серого"
    );
    // Белый точки остаётся белым при любом k.
    for gain in [0.7_f32, 1.0, 2.0, 1.41] {
        let adjusted = settings.with_exposure_gain(ExposureGain::new(gain).unwrap());
        let base = AgxBase::resolve(&adjusted, DisplayGamut::Srgb).unwrap();
        assert_eq!(base.curve_channel(1.0, 1.0), 1.0);
        assert_eq!(base.curve_channel(3.0, 3.0), 1.0);
    }
    // Без подтяжки настройки не меняются на бит.
    assert_eq!(settings.with_exposure_gain(ExposureGain::UNITY), settings);
}

#[test]
fn resolve_rejects_curves_that_cannot_reach_the_white() {
    // Запас до белого при сером 0,075 — 3,74 ступени: наклон 0,1 (< (1 − y_p)/3,74 ≈ 0,131) даёт r1 ≤ 1.
    let flat = AgxSettings {
        slope: 0.1,
        ..AgxSettings::default()
    };
    match AgxBase::resolve(&flat, DisplayGamut::Srgb) {
        Err(InvalidAgx::ShoulderTooShallow { minimum_slope, .. }) => assert!((minimum_slope - 0.1307).abs() < 1e-3),
        other => panic!("ожидали ShoulderTooShallow, получили {other:?}"),
    }
    let shallow_toe = AgxSettings {
        toe_depth_ev: 2.0,
        ..AgxSettings::default()
    };
    assert!(matches!(
        AgxBase::resolve(&shallow_toe, DisplayGamut::Srgb),
        Err(InvalidAgx::ToeTooShallow { .. })
    ));
    // Подтяжка AE, после которой серый сцены доходит до белого, — отдельная различимая ошибка.
    let bright_grey = AgxSettings {
        scene_middle_gray: 0.2,
        ..AgxSettings::default()
    }
    .with_exposure_gain(ExposureGain::new(0.2).unwrap());
    assert!(matches!(
        AgxBase::resolve(&bright_grey, DisplayGamut::Srgb),
        Err(InvalidAgx::OutOfUnitInterval {
            field: "scene_middle_gray",
            ..
        })
    ));
    let almost_white_grey = AgxSettings {
        scene_middle_gray: 0.95,
        ..AgxSettings::default()
    };
    assert!(matches!(
        AgxBase::resolve(&almost_white_grey, DisplayGamut::Srgb),
        Err(InvalidAgx::ShoulderTooShallow { .. })
    ));
    let nan = AgxSettings {
        slope: f32::NAN,
        ..AgxSettings::default()
    };
    assert!(matches!(
        AgxBase::resolve(&nan, DisplayGamut::Srgb),
        Err(InvalidAgx::NotPositive { field: "slope", .. })
    ));
}

#[test]
fn default_curve_survives_the_extreme_residual_ae_gains() {
    // Подтяжка AE ограничена ±0,5 EV (решение 72): при самом тёмном k = 2^−0,5 запас до белого наименьший.
    for ev in [-0.5_f32, 0.0, 0.5] {
        let adjusted = AgxSettings::default().with_exposure_gain(ExposureGain::from_ev(ev).unwrap());
        assert!(AgxBase::resolve(&adjusted, DisplayGamut::Srgb).is_ok(), "{ev} EV");
    }
}

#[test]
fn output_stage_converts_gamut_and_clamps() {
    let base = AgxBase::resolve(&AgxSettings::default(), DisplayGamut::DisplayP3).unwrap();
    // Серый проходит без изменений, значения вне 0…1 обрезаются, отрицательные цвета сжимаются в охват.
    let grey = base.to_output([0.4; 3]);
    assert!(grey.iter().all(|c| (c - 0.4).abs() < 2e-6), "{grey:?}");
    let out = base.to_output([1.4, -0.2, 0.3]);
    assert!(out.iter().all(|c| (0.0..=1.0).contains(c)), "{out:?}");
    assert_eq!(base.output_gamut(), DisplayGamut::DisplayP3);
}

#[test]
fn inset_column_is_the_chromaticity_of_the_moved_primary() {
    // Оракул из доки: столбец матрицы «внутрь» — координаты в W основного цвета p' = w + (1 − a)·Rot(θ)·(p − w);
    // переведённый в XYZ и в xy, он обязан лечь ровно в p'.
    use crate::color::chromaticity::Chromaticity;
    use crate::color::working_space::{PrimaryAdjustment, WORKING_PRIMARIES};
    let adjustments = [
        PrimaryAdjustment {
            amount: 0.3,
            rotation_degrees: 10.0,
        },
        PrimaryAdjustment {
            amount: 0.2,
            rotation_degrees: -15.0,
        },
        PrimaryAdjustment {
            amount: 0.45,
            rotation_degrees: 5.0,
        },
    ];
    let settings = AgxSettings {
        inset: adjustments,
        ..AgxSettings::default()
    };
    let parameters = AgxBase::resolve(&settings, DisplayGamut::Srgb).unwrap().parameters();
    let to_xyz = WORKING_PRIMARIES.rgb_to_xyz().unwrap();
    let white = WORKING_PRIMARIES.white;
    let primaries = [WORKING_PRIMARIES.red, WORKING_PRIMARIES.green, WORKING_PRIMARIES.blue];
    for (index, (primary, adjustment)) in primaries.iter().zip(adjustments).enumerate() {
        let column = [
            f64::from(parameters.inset[0][index]),
            f64::from(parameters.inset[1][index]),
            f64::from(parameters.inset[2][index]),
        ];
        let moved = Chromaticity::from_xyz(to_xyz.apply(column)).unwrap();
        let angle = f64::from(adjustment.rotation_degrees).to_radians();
        let keep = 1.0 - f64::from(adjustment.amount);
        let (dx, dy) = (primary.x - white.x, primary.y - white.y);
        let expected_x = white.x + keep * (dx * angle.cos() - dy * angle.sin());
        let expected_y = white.y + keep * (dx * angle.sin() + dy * angle.cos());
        assert!(
            (moved.x - expected_x).abs() < 2e-6 && (moved.y - expected_y).abs() < 2e-6,
            "основной цвет {index}: {moved:?}"
        );
    }
}

#[test]
fn tone_base_defaults_to_legacy_and_reads_from_config_text() {
    use crate::tone::base::ToneBase;
    assert_eq!(ToneBase::default(), ToneBase::Agx(AgxSettings::default()));
    assert_eq!(
        serde_json::from_str::<ToneBase>("\"legacy\"").unwrap(),
        ToneBase::Legacy
    );
    // Частичный конфиг: остальные числа — умолчания; выбранное значение доходит до настроек.
    let parsed: ToneBase = serde_json::from_str(r#"{"agx": {"slope": 0.25}}"#).unwrap();
    assert_eq!(
        parsed,
        ToneBase::Agx(AgxSettings {
            slope: 0.25,
            ..AgxSettings::default()
        })
    );
    // Полный круг «настройки → текст → настройки» ничего не теряет.
    let full = ToneBase::Agx(AgxSettings::default());
    assert_eq!(
        serde_json::from_str::<ToneBase>(&serde_json::to_string(&full).unwrap()).unwrap(),
        full
    );
    // Опечатка в имени числа — ошибка, а не молчаливое умолчание.
    let error = serde_json::from_str::<ToneBase>(r#"{"agx": {"slop": 0.25}}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("slop"), "{error}");
}

#[test]
fn resolve_reports_a_broken_agx_setup_as_a_distinct_resolve_error() {
    use crate::demosaic::DemosaicMethod;
    use crate::params::{FrameInputs, IspParams, ProcessingControls, ResolveError};
    use crate::sensor::SensorDescription;
    use crate::test_support::{lamp_neutral, main_camera_calibration, main_camera_passport};
    use crate::tone::base::ToneBase;
    use camera_contract::frame_passport::CaptureColorHints;
    let sensor = SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb);
    let broken = ToneBase::Agx(AgxSettings {
        slope: 0.05,
        ..AgxSettings::default()
    });
    let error = IspParams::resolve_with_base(&sensor, &frame, &controls, &broken).unwrap_err();
    assert!(
        matches!(error, ResolveError::Agx(InvalidAgx::ShoulderTooShallow { .. })),
        "{error}"
    );
    assert!(error.to_string().contains("основа AgX"));
}
