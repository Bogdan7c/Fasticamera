use std::sync::Arc;

use isp_model::cfa::BayerPattern;
use isp_model::look::LookControls;
use isp_model::look::lut::LookLut;
use isp_model::look::model::LookModelSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use look_contract::{
    BandAdjustment, BandAdjustments, ColorWheel, ColorWheels, LookKnob, LookSettings, WheelPush, WheelZone,
};

use super::*;
use crate::quality::test_params::quality_params_with_look;

pub(super) fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("значение ручки в [-1, 1]")
}

fn f32_at(bytes: &[u8], row: usize, column: usize) -> f32 {
    let at = 16 * row + 4 * column;
    f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

pub(super) fn row_at(bytes: &[u8], row: usize) -> [f32; 4] {
    [0, 1, 2, 3].map(|column| f32_at(bytes, row, column))
}

/// Вид с насыщенностью 0,6, сочностью −0,3 и одной тронутой полосой (вторая полоса модели).
fn touched_look() -> (IspParams, LookSettings) {
    let model = LookModelSettings::default();
    let band = model.bands[1].id.clone();
    let settings = LookSettings {
        saturation: knob(0.6),
        vibrance: knob(-0.3),
        bands: BandAdjustments::new(vec![BandAdjustment {
            band,
            hue: knob(0.5),
            saturation: knob(-0.4),
            luminance: knob(0.25),
        }])
        .expect("одна полоса"),
        ..LookSettings::NEUTRAL
    };
    let controls = LookControls::new(settings.clone(), Arc::new(model));
    (
        quality_params_with_look(BayerPattern::RGGB, ToneBase::Agx(AgxSettings::default()), controls),
        settings,
    )
}

#[test]
fn neutral_look_packs_all_zero_block_with_flag_off() {
    let params = quality_params_with_look(
        BayerPattern::RGGB,
        ToneBase::Agx(AgxSettings::default()),
        LookControls::default(),
    );
    assert!(params.look.color.is_none(), "нейтральные ручки — стадия пропущена");
    let bytes = pack_look_uniforms(&params);
    assert_eq!(bytes.len(), LOOK_ROWS * 16);
    assert!(bytes.iter().all(|byte| *byte == 0), "нейтраль — нули, флаг цвета 0");
}

#[test]
fn touched_look_packs_every_row_at_its_documented_place() {
    let (params, _) = touched_look();
    let color = params.look.color.expect("ручки тронуты — стадия включена");
    let bands = color.bands.expect("полоса тронута");
    let bytes = pack_look_uniforms(&params);
    assert_eq!(row_at(&bytes, 0), [1.0, bands.count as f32, 0.0, 0.0], "флаги");
    assert_eq!(
        row_at(&bytes, 1),
        [
            color.saturation_multiplier,
            color.vibrance_amount,
            color.vibrance_chroma_reference,
            color.neutral_chroma_floor
        ],
        "цветность"
    );
    assert_eq!(
        row_at(&bytes, 2),
        [
            color.skin_direction[0],
            color.skin_direction[1],
            color.skin_cos_half_width,
            color.skin_protection
        ],
        "кожа"
    );
    for (first_row, rows) in [
        (3, color.oklab.working_to_lms),
        (6, color.oklab.lms_prime_to_lab),
        (9, color.oklab.lab_to_lms_prime),
        (12, color.oklab.lms_to_working),
    ] {
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(
                row_at(&bytes, first_row + index),
                [row[0], row[1], row[2], 0.0],
                "матрица OKLab со строки {first_row}, строка {index}"
            );
        }
    }
    for (first_row, values) in [
        (15, bands.centers_radians),
        (19, bands.hue_shift_radians),
        (23, bands.chroma_multiplier),
        (27, bands.lightness_multiplier),
    ] {
        for (band, value) in values.iter().enumerate() {
            assert_eq!(
                f32_at(&bytes, first_row + band / 4, band % 4),
                *value,
                "массив полос со строки {first_row}, полоса {band}"
            );
        }
    }
    // Тронута именно вторая полоса: её сдвиг оттенка не нулевой, у первой — нуль.
    assert_ne!(f32_at(&bytes, 19, 1), 0.0);
    assert_eq!(f32_at(&bytes, 19, 0), 0.0);
}

#[test]
fn colour_without_touched_bands_packs_zero_band_count_and_neutral_band_arrays() {
    let settings = LookSettings {
        saturation: knob(0.3),
        ..LookSettings::NEUTRAL
    };
    let controls = LookControls::new(settings, Arc::new(LookModelSettings::default()));
    let params = quality_params_with_look(BayerPattern::RGGB, ToneBase::Agx(AgxSettings::default()), controls);
    let color = params.look.color.expect("насыщенность тронута");
    assert!(color.bands.is_none());
    let bytes = pack_look_uniforms(&params);
    assert_eq!(row_at(&bytes, 0), [1.0, 0.0, 0.0, 0.0], "цвет включён, полос нет");
    assert!(bytes[15 * 16..].iter().all(|byte| *byte == 0), "массивы полос — нули");
}

#[test]
fn lut_in_params_does_not_change_the_colour_block() {
    // Таблица живёт в своём буфере: блок цвета у нейтрального вида с таблицей остаётся нулевым.
    let lut = LookLut::identity(5, 65).expect("таблица 5³");
    let controls = LookControls::default().with_lut(lut);
    let params = quality_params_with_look(BayerPattern::RGGB, ToneBase::Agx(AgxSettings::default()), controls);
    assert!(params.look.lut.is_some());
    assert!(pack_look_uniforms(&params).iter().all(|byte| *byte == 0));
}

/// Параметры качественного пути с настройками вида `settings` и моделью по умолчанию.
fn params_for(settings: LookSettings) -> IspParams {
    let controls = LookControls::new(settings, Arc::new(LookModelSettings::default()));
    quality_params_with_look(BayerPattern::RGGB, ToneBase::Agx(AgxSettings::default()), controls)
}

fn wheel(hue_degrees: f32, strength: f32, ring: f32) -> ColorWheel {
    ColorWheel {
        push: WheelPush::from_hue_degrees(hue_degrees, strength).expect("толчок в круге"),
        ring: knob(ring),
    }
}

/// Строки 31–35 блока (колёса) как массив строк.
fn wheel_rows(bytes: &[u8]) -> [[f32; 4]; 5] {
    [31, 32, 33, 34, 35].map(|row| row_at(bytes, row))
}

#[test]
fn block_has_a_row_for_every_wheel_and_the_shader_agrees_on_the_row_count() {
    assert_eq!(LOOK_ROWS, 51);
    let shader = include_str!("../../shaders/look_stage.glsl");
    assert!(
        shader.contains(&format!("#define LOOK_ROWS {LOOK_ROWS}\n")),
        "число строк блока в GLSL разошлось с Rust"
    );
    assert!(
        shader.contains("const uint LOOK_ROW_WHEELS = 31u;"),
        "строка колёс в GLSL не 31"
    );
    assert!(
        shader.contains("const uint LOOK_ROW_TONE = 36u;"),
        "строка тона в GLSL не 36"
    );
    assert!(
        shader.contains("const uint LOOK_ROW_PRIMARIES = 39u;"),
        "строка полосок в GLSL не 39"
    );
}

#[test]
fn wheels_pack_the_resolved_numbers_into_their_own_rows_in_lift_gamma_gain_offset_order() {
    let wheels = ColorWheels::NEUTRAL
        .with_wheel(WheelZone::Lift, wheel(30.0, 0.9, 0.5))
        .with_wheel(WheelZone::Gamma, wheel(120.0, 0.4, -0.3))
        .with_wheel(WheelZone::Gain, wheel(250.0, 0.7, 1.0))
        .with_wheel(WheelZone::Offset, wheel(330.0, 0.2, -1.0));
    let params = params_for(LookSettings {
        wheels,
        ..LookSettings::NEUTRAL
    });
    let resolved = params
        .look
        .color
        .expect("колёса тронуты — цвет включён")
        .wheels
        .expect("колёса не нейтральны");
    let bytes = pack_look_uniforms(&params);
    assert_eq!(
        row_at(&bytes, 0),
        [1.0, 0.0, 0.0, 0.0],
        "колёса одни: флаг цвета 1, полос нет"
    );
    assert_eq!(
        row_at(&bytes, 31),
        [
            1.0,
            resolved.pivot_lightness,
            resolved.shadow_window_scale,
            resolved.highlight_window_scale
        ]
    );
    for (row, shift) in [
        (32, resolved.lift),
        (33, resolved.gamma),
        (34, resolved.gain),
        (35, resolved.offset),
    ] {
        assert_eq!(row_at(&bytes, row), [shift[0], shift[1], shift[2], 0.0], "строка {row}");
    }
    // Колёса разные: перепутанные строки не пройдут (и сами числа не нулевые).
    let rows = wheel_rows(&bytes);
    for first in 1..5 {
        for second in first + 1..5 {
            assert_ne!(rows[first], rows[second], "строки {first} и {second} совпали");
        }
        assert_ne!(rows[first][..3], [0.0; 3], "строка {first} пустая");
    }
    // Кольцо Lift +0,5 → ΔL положительна, Gamma −0,3 → отрицательна: знак попал в свой слот.
    assert!(rows[1][0] > 0.0 && rows[2][0] < 0.0 && rows[3][0] > 0.0 && rows[4][0] < 0.0);
}

#[test]
fn wheel_zero_pushes_leave_the_zone_row_zero_while_other_zones_keep_their_numbers() {
    // Только Gain тронут: остальные три строки зон — нули, окно зон включено.
    let params = params_for(LookSettings {
        wheels: ColorWheels::NEUTRAL.with_wheel(WheelZone::Gain, wheel(90.0, 1.0, 0.0)),
        ..LookSettings::NEUTRAL
    });
    let bytes = pack_look_uniforms(&params);
    let rows = wheel_rows(&bytes);
    assert_eq!(rows[0][0], 1.0);
    assert_eq!(rows[1], [0.0; 4], "Lift не тронут");
    assert_eq!(rows[2], [0.0; 4], "Gamma не тронута");
    assert_ne!(rows[3], [0.0; 4], "Gain тронут");
    assert_eq!(rows[4], [0.0; 4], "Offset не тронут");
}

#[test]
fn neutral_wheels_pack_zero_rows_and_leave_the_colour_rows_exactly_as_without_wheels() {
    let saturation_only = LookSettings {
        saturation: knob(0.6),
        ..LookSettings::NEUTRAL
    };
    let plain = pack_look_uniforms(&params_for(saturation_only.clone()));
    assert!(
        wheel_rows(&plain).iter().all(|row| *row == [0.0; 4]),
        "нейтральные колёса — нули"
    );
    assert_eq!(row_at(&plain, 0)[0], 1.0, "цвет включён насыщенностью");

    // Колёса тронули и вернули в нейтраль: блок бит в бит как у вида, где их не трогали.
    let touched_then_reset = LookSettings {
        wheels: ColorWheels::NEUTRAL
            .with_wheel(WheelZone::Lift, wheel(30.0, 1.0, 1.0))
            .with_wheel(WheelZone::Lift, ColorWheel::NEUTRAL),
        ..saturation_only.clone()
    };
    assert_eq!(pack_look_uniforms(&params_for(touched_then_reset)), plain);

    // Строки 0–30 с колёсами те же, что без них: колёса их не трогают.
    let with_wheels = pack_look_uniforms(&params_for(LookSettings {
        wheels: ColorWheels::NEUTRAL.with_wheel(WheelZone::Gain, wheel(200.0, 0.8, 0.4)),
        ..saturation_only
    }));
    assert_eq!(
        with_wheels[..31 * 16],
        plain[..31 * 16],
        "строки 0–30 изменились из-за колёс"
    );
    assert_ne!(with_wheels[31 * 16..], plain[31 * 16..]);
}

/// Ручки тона: контраст 0,5, средние −0,4, тени 0,8, света −0,6 — все четыре наклона различны и не единичны.
fn tone_only_settings() -> LookSettings {
    LookSettings {
        contrast: knob(0.5),
        midtones: knob(-0.4),
        shadows: knob(0.8),
        highlights: knob(-0.6),
        ..LookSettings::NEUTRAL
    }
}

/// Строки тона 36–38 блока.
fn tone_rows(bytes: &[u8]) -> [[f32; 4]; 3] {
    [36, 37, 38].map(|row| row_at(bytes, row))
}

#[test]
fn tone_only_packs_its_rows_at_the_documented_places_and_leaves_colour_rows_zero() {
    let params = params_for(tone_only_settings());
    assert!(params.look.color.is_none(), "цвет не тронут");
    let tone = params.look.tone.expect("ручки тона тронуты");
    let bytes = pack_look_uniforms(&params);
    assert!(
        bytes[..COLOR_ROWS * 16].iter().all(|byte| *byte == 0),
        "тон один: строки 0–35 (цвет, колёса) — нули"
    );
    assert_eq!(
        tone_rows(&bytes),
        [
            [1.0, tone.pivot, tone.shadow_scale, tone.highlight_scale],
            [
                tone.shadow_slope,
                tone.highlight_slope,
                tone.contrast_slope,
                tone.midtone_slope
            ],
            [tone.color_follow, 0.0, 0.0, 0.0],
        ]
    );
    // Числа не нулевые и не единичные: перепутанный слот не спрячется за совпадением.
    let slopes = row_at(&bytes, 37);
    for (index, slope) in slopes.iter().enumerate() {
        assert!(*slope > 0.0 && (*slope - 1.0).abs() > 0.05, "наклон {index} = {slope}");
        for other in slopes.iter().skip(index + 1) {
            assert_ne!(slope, other, "два наклона тона совпали");
        }
    }
    // Знаки ручек попали в свои слоты: тени + → круче 1, света − → у белого меньше 1 (плюс даёт меньше), средние − → меньше 1.
    assert!(slopes[0] > 1.0, "тени +0,8: наклон у чёрного больше 1");
    assert!(slopes[1] > 1.0, "света −0,6: расстояние до белого растянуто");
    assert!(slopes[2] > 1.0, "контраст +0,5");
    assert!(slopes[3] < 1.0, "средние −0,4");
}

#[test]
fn colour_only_leaves_tone_rows_zero_so_the_shader_skips_the_tone_stage() {
    let params = params_for(LookSettings {
        saturation: knob(0.6),
        ..LookSettings::NEUTRAL
    });
    assert!(params.look.tone.is_none(), "ручки тона в нуле");
    let bytes = pack_look_uniforms(&params);
    assert_eq!(row_at(&bytes, 0)[0], 1.0, "цвет включён");
    assert!(
        bytes[COLOR_ROWS * 16..].iter().all(|byte| *byte == 0),
        "цвет один: строки 36–38 — нули, флаг тона 0"
    );
}

#[test]
fn tone_and_colour_together_do_not_disturb_each_other() {
    let colour = LookSettings {
        saturation: knob(0.6),
        ..LookSettings::NEUTRAL
    };
    let both = LookSettings {
        saturation: knob(0.6),
        ..tone_only_settings()
    };
    let colour_bytes = pack_look_uniforms(&params_for(colour));
    let tone_bytes = pack_look_uniforms(&params_for(tone_only_settings()));
    let both_bytes = pack_look_uniforms(&params_for(both));
    assert_eq!(
        both_bytes[..COLOR_ROWS * 16],
        colour_bytes[..COLOR_ROWS * 16],
        "тон изменил строки цвета"
    );
    assert_eq!(
        both_bytes[COLOR_ROWS * 16..],
        tone_bytes[COLOR_ROWS * 16..],
        "цвет изменил строки тона"
    );
    assert_ne!(colour_bytes, both_bytes);
}

#[test]
fn tone_rows_follow_the_model_edges_and_the_pivot_gray() {
    // Край ручки — в модели: наклон контраста = 2^(ход · край), а не число, зашитое в упаковщик.
    let mut model = LookModelSettings::default();
    model.tone.contrast = isp_model::look::range::KnobRange::symmetric(1.0);
    model.tone.pivot_display_gray = 0.25;
    let controls = LookControls::new(
        LookSettings {
            contrast: knob(1.0),
            ..LookSettings::NEUTRAL
        },
        Arc::new(model),
    );
    let params = quality_params_with_look(BayerPattern::RGGB, ToneBase::Agx(AgxSettings::default()), controls);
    let rows = tone_rows(&pack_look_uniforms(&params));
    assert_eq!(rows[0], [1.0, 0.5, 2.0, 2.0], "q = √0,25 = 0,5, 1/q = 2, 1/(1−q) = 2");
    assert_eq!(rows[1], [1.0, 1.0, 2.0, 1.0], "только контраст ×2 на краю 1 log2");
}

#[test]
fn tone_returned_to_neutral_packs_the_same_block_as_never_touched() {
    let colour = LookSettings {
        saturation: knob(0.6),
        ..LookSettings::NEUTRAL
    };
    let toned_then_reset = LookSettings {
        contrast: knob(0.0),
        ..colour.clone()
    };
    assert_eq!(
        pack_look_uniforms(&params_for(toned_then_reset)),
        pack_look_uniforms(&params_for(colour)),
        "нулевая ручка тона — те же байты, что и нетронутая"
    );
}

#[path = "primaries_tests.rs"]
mod primaries_tests;
