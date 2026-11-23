use std::sync::Arc;

use isp_model::cfa::BayerPattern;
use isp_model::look::LookControls;
use isp_model::look::lut::LookLut;
use isp_model::look::model::LookModelSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use look_contract::{BandAdjustment, BandAdjustments, LookKnob, LookSettings};

use super::*;
use crate::quality::test_params::quality_params_with_look;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("значение ручки в [-1, 1]")
}

fn f32_at(bytes: &[u8], row: usize, column: usize) -> f32 {
    let at = 16 * row + 4 * column;
    f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn row_at(bytes: &[u8], row: usize) -> [f32; 4] {
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
