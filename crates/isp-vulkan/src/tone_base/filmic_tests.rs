//! Тесты упаковщика основы filmic (раскладка строк, номер модели, помещение в блок). Паритет GPU с эталоном —
//! в `tests/*_filmic_parity.rs` и `tests/photo_filmic.rs`.

use isp_model::tone::filmic::{FilmicNorm, FilmicParameters, FilmicSettings};

use super::*;

/// Числа filmic для настроек `settings` (параметры кадра по умолчанию, обрезка нулём на входе).
fn filmic_params(settings: FilmicSettings) -> (IspParams, FilmicParameters) {
    let params = params_with(ToneBase::Filmic(settings));
    let ResolvedToneBase::Filmic(base) = &params.tone_base else {
        panic!("ожидалась основа filmic");
    };
    let numbers = base.parameters();
    (params, numbers)
}

#[test]
fn filmic_params_select_filmic_kind_with_its_own_model_id() {
    let (params, _) = filmic_params(FilmicSettings::default());
    assert_eq!(ToneBaseKind::of(&params), ToneBaseKind::Filmic);
    // Номер 3 — зеркало `TONE_MODEL_FILMIC` в `tone_base.glsl`; 1 и 2 заняты AgX и ACES.
    assert_eq!(ToneBaseKind::Filmic.model_id(), 3);
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(f32_at(&bytes, 12), 3.0, "строка 0, w: номер модели");
}

#[test]
fn layout_matches_documented_rows() {
    let (params, filmic) = filmic_params(FilmicSettings::default());
    let bytes = pack_tone_base_uniforms(&params);
    let gains = params.white_balance.as_array();
    assert_row(&bytes, 0, [gains[0], gains[1], gains[2], 3.0], "общая строка 0");
    assert_matrix_at(&bytes, 0, filmic.xyz_d50_to_working, "XYZ → W");
    assert_matrix_at(&bytes, 3, filmic.working_to_output, "W → выход");
    assert_row(
        &bytes,
        model_row(6),
        [filmic.grey, filmic.gray_output, filmic.gamma, filmic.slope],
        "серый и наклон",
    );
    assert_row(
        &bytes,
        model_row(7),
        [
            filmic.linear_below,
            filmic.toe_value,
            filmic.toe_width,
            filmic.toe_excess,
        ],
        "носок",
    );
    assert_row(
        &bytes,
        model_row(8),
        [
            filmic.toe_power,
            filmic.toe_inverse_power,
            filmic.shoulder_power,
            filmic.shoulder_inverse_power,
        ],
        "степени",
    );
    assert_row(
        &bytes,
        model_row(9),
        [
            filmic.linear_above,
            filmic.shoulder_value,
            filmic.shoulder_headroom,
            filmic.desaturation_onset,
        ],
        "плечо и порог обесцвечивания",
    );
    // Вход по умолчанию — обрезка нулём: флаг 0 и безопасные числа.
    assert_row(&bytes, model_row(10), [0.0, 0.0, 0.0, 1.0], "вход: пороги");
    assert_row(&bytes, model_row(11), [1.0; 4], "вход: масштабы");
    let output = filmic.output_compression;
    assert_row(
        &bytes,
        model_row(12),
        [
            output.threshold[0],
            output.threshold[1],
            output.threshold[2],
            output.power,
        ],
        "выход: пороги",
    );
    assert_row(
        &bytes,
        model_row(13),
        [output.scale[0], output.scale[1], output.scale[2], 1.0 / output.power],
        "выход: масштабы",
    );
    // Норма по умолчанию — степенная (код 3).
    assert_eq!(filmic.norm, FilmicNorm::PowerNorm);
    assert_row(&bytes, model_row(14), [0.0, 3.0, 0.0, 0.0], "флаги: вход, норма");
    assert_row(
        &bytes,
        model_row(15),
        [
            filmic.luminance_row[0],
            filmic.luminance_row[1],
            filmic.luminance_row[2],
            0.0,
        ],
        "строка яркости",
    );
    assert_matrix_at(&bytes, 16, filmic.oklab.working_to_lms, "OKLab W → LMS");
    assert_matrix_at(&bytes, 19, filmic.oklab.lms_prime_to_lab, "OKLab LMS' → Lab");
    assert_matrix_at(&bytes, 22, filmic.oklab.lab_to_lms_prime, "OKLab Lab → LMS'");
    assert_matrix_at(&bytes, 25, filmic.oklab.lms_to_working, "OKLab LMS → W");
    assert!(
        bytes[model_row(super::filmic::ROW_COUNT)..]
            .iter()
            .all(|byte| *byte == 0),
        "хвост блока за строками модели нулевой"
    );
}

#[test]
fn every_norm_has_its_own_code_in_the_flags_row() {
    for (norm, code) in [
        (FilmicNorm::PerChannel, 0.0),
        (FilmicNorm::MaxRgb, 1.0),
        (FilmicNorm::Luminance, 2.0),
        (FilmicNorm::PowerNorm, 3.0),
    ] {
        let (params, filmic) = filmic_params(FilmicSettings {
            norm,
            ..FilmicSettings::default()
        });
        assert_eq!(filmic.norm, norm);
        let bytes = pack_tone_base_uniforms(&params);
        assert_eq!(f32_at(&bytes, model_row(14) + 4), code, "{norm:?}");
    }
}

#[test]
fn soft_compress_input_is_packed_with_its_thresholds_and_flag() {
    let settings = FilmicSettings {
        input_gamut: WorkingInputGamut::SoftCompress(GamutCompressionSettings {
            threshold: [0.7, 0.8, 0.9],
            distance_limit: [1.3, 1.4, 1.5],
            power: 1.5,
        }),
        ..FilmicSettings::default()
    };
    let (params, filmic) = filmic_params(settings);
    let compression = filmic.input_compression.expect("мягкое сжатие");
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(f32_at(&bytes, model_row(14)), 1.0, "флаг «вход — мягкое сжатие»");
    for channel in 0..3 {
        assert_eq!(
            f32_at(&bytes, model_row(10) + 4 * channel),
            compression.threshold[channel]
        );
        assert_eq!(f32_at(&bytes, model_row(11) + 4 * channel), compression.scale[channel]);
    }
    assert_eq!(f32_at(&bytes, model_row(10) + 12), 1.5);
    assert_eq!(f32_at(&bytes, model_row(11) + 12), 1.0 / 1.5);
}

/// Упаковщик пишет ровно заявленное число строк и помещается в блок для каждой нормы.
#[test]
fn filmic_writes_exactly_its_row_count_and_fits_the_block() {
    for norm in [
        FilmicNorm::PerChannel,
        FilmicNorm::MaxRgb,
        FilmicNorm::Luminance,
        FilmicNorm::PowerNorm,
    ] {
        let (_, filmic) = filmic_params(FilmicSettings {
            norm,
            ..FilmicSettings::default()
        });
        let mut rows = ToneBaseRows::default();
        super::filmic::pack(&mut rows, &filmic);
        assert_eq!(rows.len(), super::filmic::ROW_COUNT, "{norm:?}");
        assert!(FIRST_MODEL_ROW + rows.len() <= TONE_BASE_ROWS_MAX, "{norm:?}");
    }
}
