use isp_model::gamut::GamutCompressionSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::working_io::WorkingInputGamut;

use super::*;

use crate::quality::test_params::quality_params_with_base;
use isp_model::cfa::BayerPattern;

fn params_with(base: ToneBase) -> IspParams {
    quality_params_with_base(BayerPattern::RGGB, base)
}

fn agx_params(settings: AgxSettings) -> (IspParams, isp_model::tone::agx::AgxParameters) {
    let params = params_with(ToneBase::Agx(settings));
    let ResolvedToneBase::Agx(base) = &params.tone_base else {
        panic!("ожидалась основа AgX");
    };
    let numbers = base.parameters();
    (params, numbers)
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_ne_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

/// Смещение строки модели: строка 0 блока общая, числа модели идут с `FIRST_MODEL_ROW`.
fn model_row(offset_in_model: usize) -> usize {
    16 * (FIRST_MODEL_ROW + offset_in_model)
}

fn assert_row(bytes: &[u8], at: usize, expected: [f32; 4], what: &str) {
    for (index, value) in expected.iter().enumerate() {
        assert_eq!(f32_at(bytes, at + 4 * index), *value, "{what}: число {index}");
    }
}

fn assert_matrix_at(bytes: &[u8], first_row: usize, rows: [[f32; 3]; 3], what: &str) {
    for (row, values) in rows.iter().enumerate() {
        assert_row(
            bytes,
            model_row(first_row + row),
            [values[0], values[1], values[2], 0.0],
            what,
        );
    }
}

#[test]
fn layout_matches_documented_rows() {
    let (params, agx) = agx_params(AgxSettings {
        hue_restoration: 0.0,
        ..AgxSettings::default()
    });
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(bytes.len(), TONE_BASE_ROWS_MAX * 16);
    let gains = params.white_balance.as_array();
    assert_row(
        &bytes,
        0,
        [gains[0], gains[1], gains[2], 1.0],
        "общая строка 0: баланс и номер модели",
    );
    assert_matrix_at(&bytes, 0, agx.xyz_d50_to_working, "XYZ → W");
    assert_matrix_at(&bytes, 3, agx.inset, "inset");
    assert_matrix_at(&bytes, 6, agx.outset, "outset");
    assert_matrix_at(&bytes, 9, agx.working_to_output, "W → выход");
    assert_row(
        &bytes,
        model_row(12),
        [agx.grey, agx.gray_output, agx.gamma, agx.slope],
        "кривая: серый",
    );
    assert_row(
        &bytes,
        model_row(13),
        [agx.toe_power, agx.toe_inverse_power, agx.toe_depth, agx.toe_excess],
        "носок",
    );
    assert_row(
        &bytes,
        model_row(14),
        [agx.shoulder_power, agx.shoulder_inverse_power, agx.shoulder_scale, 0.0],
        "плечо",
    );
    // Вход по умолчанию — обрезка нулём: флаг 0 и безопасные числа.
    assert_row(&bytes, model_row(15), [0.0, 0.0, 0.0, 1.0], "вход: пороги");
    assert_row(&bytes, model_row(16), [1.0; 4], "вход: масштабы");
    let output = agx.output_compression;
    assert_row(
        &bytes,
        model_row(17),
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
        model_row(18),
        [output.scale[0], output.scale[1], output.scale[2], 1.0 / output.power],
        "выход: масштабы",
    );
    assert_row(
        &bytes,
        model_row(19),
        [0.0; 4],
        "флаги: вход — обрезка, возврат оттенка выключен",
    );
    assert!(
        bytes[model_row(20)..].iter().all(|byte| *byte == 0),
        "без возврата оттенка строки 20… и хвост блока нулевые"
    );
}

#[test]
fn hue_restoration_rows_carry_degree_floor_luminance_and_oklab_matrices() {
    let (params, agx) = agx_params(AgxSettings {
        hue_restoration: 0.5,
        ..AgxSettings::default()
    });
    let hue = agx.hue_restoration.expect("степень 0,5 включает возврат оттенка");
    let bytes = pack_tone_base_uniforms(&params);
    assert_row(
        &bytes,
        model_row(19),
        [0.0, 0.5, hue.chroma_floor, 0.0],
        "флаги возврата оттенка",
    );
    assert_matrix_at(&bytes, 20, hue.oklab.working_to_lms, "OKLab W → LMS");
    assert_row(
        &bytes,
        model_row(23),
        [hue.luminance_row[0], hue.luminance_row[1], hue.luminance_row[2], 0.0],
        "строка яркости",
    );
    assert_matrix_at(&bytes, 24, hue.oklab.lms_prime_to_lab, "OKLab LMS' → Lab");
    assert_matrix_at(&bytes, 27, hue.oklab.lab_to_lms_prime, "OKLab Lab → LMS'");
    assert_matrix_at(&bytes, 30, hue.oklab.lms_to_working, "OKLab LMS → W");
    // Матрицы OKLab — не нули: серый (1, 1, 1) переходит в равные LMS (строки нормированы на отклик белого).
    let first_row = hue.oklab.working_to_lms[0];
    assert!(
        (first_row[0] + first_row[1] + first_row[2] - 1.0).abs() < 1e-5,
        "{first_row:?}"
    );
}

#[test]
fn soft_compress_input_is_packed_with_its_thresholds_and_flag() {
    let settings = AgxSettings {
        input_gamut: WorkingInputGamut::SoftCompress(GamutCompressionSettings {
            threshold: [0.7, 0.8, 0.9],
            distance_limit: [1.3, 1.4, 1.5],
            power: 1.5,
        }),
        ..AgxSettings::default()
    };
    let (params, agx) = agx_params(settings);
    let compression = agx.input_compression.expect("мягкое сжатие");
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(f32_at(&bytes, model_row(19)), 1.0, "флаг «вход — мягкое сжатие»");
    for channel in 0..3 {
        assert_eq!(
            f32_at(&bytes, model_row(15) + 4 * channel),
            compression.threshold[channel]
        );
        assert_eq!(f32_at(&bytes, model_row(16) + 4 * channel), compression.scale[channel]);
    }
    assert_eq!(f32_at(&bytes, model_row(15) + 12), 1.5);
    assert_eq!(f32_at(&bytes, model_row(16) + 12), 1.0 / 1.5);
}

#[test]
fn legacy_base_packs_only_the_common_row() {
    let params = params_with(ToneBase::Legacy);
    let bytes = pack_tone_base_uniforms(&params);
    let gains = params.white_balance.as_array();
    assert_row(
        &bytes,
        0,
        [gains[0], gains[1], gains[2], 0.0],
        "общая строка: баланс и номер 0",
    );
    assert!(
        bytes[16..].iter().all(|byte| *byte == 0),
        "у прежней основы чисел модели нет — нули"
    );
    assert_eq!(ToneBaseKind::of(&params), ToneBaseKind::Legacy);
    assert_eq!(ToneBaseKind::Legacy.model_id(), 0);
}

#[test]
fn agx_params_select_agx_kind() {
    let (params, _) = agx_params(AgxSettings::default());
    assert_eq!(ToneBaseKind::of(&params), ToneBaseKind::Agx);
    assert_eq!(ToneBaseKind::Agx.model_id(), 1);
}

/// Каждая модель пишет ровно заявленное число строк и помещается в блок вместе с общей строкой: добавление
/// модели, которая выходит за `TONE_BASE_ROWS_MAX`, ломается здесь, а не молчаливой потерей хвоста.
#[test]
fn every_model_writes_exactly_its_row_count_and_fits_the_block() {
    for (what, settings) in [
        (
            "AgX без возврата оттенка",
            AgxSettings {
                hue_restoration: 0.0,
                ..AgxSettings::default()
            },
        ),
        (
            "AgX с возвратом оттенка",
            AgxSettings {
                hue_restoration: 1.0,
                ..AgxSettings::default()
            },
        ),
    ] {
        let (_, agx) = agx_params(settings);
        let mut rows = ToneBaseRows::default();
        agx::pack(&mut rows, &agx);
        assert_eq!(rows.len(), agx::ROW_COUNT, "{what}");
        assert!(FIRST_MODEL_ROW + rows.len() <= TONE_BASE_ROWS_MAX, "{what}");
    }
    let mut rows = ToneBaseRows::default();
    legacy::pack(&mut rows);
    assert_eq!(rows.len(), legacy::ROW_COUNT);
}

/// Все четыре модели основы поддержаны на каждой точке выбора: номер модели, проверка запроса и упаковка блока
/// (молчаливой подмены на AgX нет, отказов не осталось). Номера моделей различны: по ним шейдер выбирает формулу.
#[test]
fn every_model_is_supported_at_every_selection_point_with_a_distinct_model_id() {
    use isp_model::tone::aces::AcesSettings;
    use isp_model::tone::filmic::FilmicSettings;
    let mut model_ids = Vec::new();
    for (base, kind) in [
        (ToneBase::Legacy, ToneBaseKind::Legacy),
        (ToneBase::Agx(AgxSettings::default()), ToneBaseKind::Agx),
        (ToneBase::Aces(AcesSettings::default()), ToneBaseKind::Aces),
        (ToneBase::Filmic(FilmicSettings::default()), ToneBaseKind::Filmic),
    ] {
        let params = params_with(base);
        assert_eq!(ToneBaseKind::of(&params), kind);
        let bytes = pack_tone_base_uniforms(&params);
        assert_eq!(
            f32_at(&bytes, 12),
            kind.model_id() as f32,
            "{kind:?}: номер модели в строке 0"
        );
        model_ids.push(kind.model_id());
    }
    model_ids.sort_unstable();
    model_ids.dedup();
    assert_eq!(model_ids.len(), 4, "номера моделей должны быть различны");
}

#[path = "aces_tests.rs"]
mod aces_tests;

#[path = "filmic_tests.rs"]
mod filmic_tests;
