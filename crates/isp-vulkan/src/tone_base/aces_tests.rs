//! Тесты упаковщика основы ACES (раскладка строк, номер модели, помещение в блок). Паритет GPU с эталоном —
//! в `tests/*_aces_parity.rs` и `tests/photo_aces.rs`.

use isp_model::gamut::GamutCompressionSettings;
use isp_model::tone::aces::spline::SplineParameters;
use isp_model::tone::aces::{AcesParameters, AcesSettings};
use isp_model::tone::working_io::WorkingInputGamut;

use super::*;

/// Числа ACES для настроек `settings` (параметры кадра по умолчанию).
fn aces_params(settings: AcesSettings) -> (IspParams, AcesParameters) {
    let params = params_with(ToneBase::Aces(settings));
    let ResolvedToneBase::Aces(base) = &params.tone_base else {
        panic!("ожидалась основа ACES");
    };
    let numbers = base.parameters();
    (params, numbers)
}

/// Проверяет девять строк сплайна, начиная со смещения `first` от первой строки модели.
fn assert_spline_at(bytes: &[u8], first: usize, spline: &SplineParameters, what: &str) {
    let mut padded_low = [0.0_f32; 12];
    padded_low[..10].copy_from_slice(&spline.coefficients_low);
    let mut padded_high = [0.0_f32; 12];
    padded_high[..10].copy_from_slice(&spline.coefficients_high);
    for row in 0..3 {
        let low: [f32; 4] = padded_low[4 * row..4 * row + 4].try_into().unwrap();
        let high: [f32; 4] = padded_high[4 * row..4 * row + 4].try_into().unwrap();
        assert_row(
            bytes,
            model_row(first + row),
            low,
            &format!("{what}: коэффициенты низа {row}"),
        );
        assert_row(
            bytes,
            model_row(first + 3 + row),
            high,
            &format!("{what}: коэффициенты верха {row}"),
        );
    }
    assert_row(
        bytes,
        model_row(first + 6),
        [spline.log_min_x, spline.log_mid_x, spline.log_max_x, spline.low_scale],
        &format!("{what}: точки"),
    );
    assert_row(
        bytes,
        model_row(first + 7),
        [
            spline.high_scale,
            spline.last_segment as f32,
            spline.slope_low,
            spline.slope_high,
        ],
        &format!("{what}: наклоны"),
    );
    assert_row(
        bytes,
        model_row(first + 8),
        [spline.low_intercept, spline.high_intercept, 0.0, 0.0],
        &format!("{what}: свободные члены"),
    );
}

#[test]
fn aces_params_select_aces_kind_with_its_own_model_id() {
    let (params, _) = aces_params(AcesSettings::default());
    assert_eq!(ToneBaseKind::of(&params), ToneBaseKind::Aces);
    // Номер 2 — зеркало `TONE_MODEL_ACES` в `tone_base.glsl`; 1 занят AgX.
    assert_eq!(ToneBaseKind::Aces.model_id(), 2);
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(f32_at(&bytes, 12), 2.0, "строка 0, w: номер модели");
}

#[test]
fn layout_matches_documented_rows() {
    let (params, aces) = aces_params(AcesSettings::default());
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(bytes.len(), TONE_BASE_ROWS_MAX * 16);
    let gains = params.white_balance.as_array();
    assert_row(&bytes, 0, [gains[0], gains[1], gains[2], 2.0], "общая строка 0");
    assert_matrix_at(&bytes, 0, aces.xyz_d50_to_working, "XYZ → W");
    assert_matrix_at(&bytes, 3, aces.working_to_output, "W → выход");
    assert_matrix_at(&bytes, 6, aces.working_to_ap0, "W → AP0");
    assert_matrix_at(&bytes, 9, aces.ap0_to_ap1, "AP0 → AP1");
    assert_matrix_at(&bytes, 12, aces.ap1_to_working, "AP1 → W");
    assert_row(
        &bytes,
        model_row(15),
        [
            aces.ap1_luminance[0],
            aces.ap1_luminance[1],
            aces.ap1_luminance[2],
            aces.exposure_scale,
        ],
        "яркость AP1 и множитель входа",
    );
    assert_row(
        &bytes,
        model_row(16),
        [
            aces.sweeteners.glow_gain,
            aces.sweeteners.red_modifier_scale,
            aces.rrt_saturation,
            aces.surround_exponent,
        ],
        "свечение, красный, десатурация RRT, окружение",
    );
    // Вход по умолчанию — обрезка нулём: флаг 0 и безопасные числа.
    assert_row(&bytes, model_row(17), [0.0; 4], "флаги: вход — обрезка");
    assert_row(&bytes, model_row(18), [0.0, 0.0, 0.0, 1.0], "вход: пороги");
    assert_row(&bytes, model_row(19), [1.0; 4], "вход: масштабы");
    let output = aces.output_compression;
    assert_row(
        &bytes,
        model_row(20),
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
        model_row(21),
        [output.scale[0], output.scale[1], output.scale[2], 1.0 / output.power],
        "выход: масштабы",
    );
    let tone = &aces.tone;
    assert_row(
        &bytes,
        model_row(22),
        [tone.grey, tone.contrast, tone.shadows_ev, tone.highlights_ev],
        "кривая: серый и ручки",
    );
    assert_row(
        &bytes,
        model_row(23),
        [tone.shape_width_ev, tone.log_reference, tone.shift_ev, tone.knee_ev],
        "кривая: ширина, опора, сдвиг, колено",
    );
    assert_row(
        &bytes,
        model_row(24),
        [tone.knee_value, tone.knee_slope, tone.max_white_ev, tone.black_level],
        "кривая: колено, предел белого, чёрный",
    );
    assert_row(
        &bytes,
        model_row(25),
        [tone.inverse_range, tone.log_black, 0.0, 0.0],
        "кривая: нормировка",
    );
    assert_spline_at(&bytes, 26, &tone.rrt, "сплайн RRT");
    assert_spline_at(&bytes, 26 + aces::SPLINE_ROW_COUNT, &tone.odt, "сплайн ODT");
    assert!(
        bytes[model_row(aces::ROW_COUNT)..].iter().all(|byte| *byte == 0),
        "за последней строкой модели блок нулевой"
    );
}

#[test]
fn soft_compress_input_is_packed_with_its_thresholds_and_flag() {
    let settings = AcesSettings {
        input_gamut: WorkingInputGamut::SoftCompress(GamutCompressionSettings {
            threshold: [0.7, 0.8, 0.9],
            distance_limit: [1.3, 1.4, 1.5],
            power: 1.5,
        }),
        ..AcesSettings::default()
    };
    let (params, aces) = aces_params(settings);
    let compression = aces.input_compression.expect("мягкое сжатие");
    let bytes = pack_tone_base_uniforms(&params);
    assert_eq!(f32_at(&bytes, model_row(17)), 1.0, "флаг «вход — мягкое сжатие»");
    for channel in 0..3 {
        assert_eq!(
            f32_at(&bytes, model_row(18) + 4 * channel),
            compression.threshold[channel]
        );
        assert_eq!(f32_at(&bytes, model_row(19) + 4 * channel), compression.scale[channel]);
    }
    assert_eq!(f32_at(&bytes, model_row(18) + 12), 1.5);
    assert_eq!(f32_at(&bytes, model_row(19) + 12), 1.0 / 1.5);
}

/// Ручки модели доходят до блока: другой контраст и свечение дают другие числа ровно в своих строках.
#[test]
fn knobs_change_exactly_their_own_rows() {
    let (base_params, _) = aces_params(AcesSettings::default());
    let (tuned_params, tuned) = aces_params(AcesSettings {
        contrast: 1.2,
        glow_gain: 0.0,
        ..AcesSettings::default()
    });
    let base = pack_tone_base_uniforms(&base_params);
    let tuned_bytes = pack_tone_base_uniforms(&tuned_params);
    assert_eq!(f32_at(&tuned_bytes, model_row(16)), 0.0, "свечение выключено");
    assert_eq!(f32_at(&tuned_bytes, model_row(22) + 4), tuned.tone.contrast);
    assert_ne!(
        f32_at(&tuned_bytes, model_row(22) + 4),
        f32_at(&base, model_row(22) + 4)
    );
    // Матрицы цвета от ручек не зависят.
    assert_eq!(base[..model_row(15)], tuned_bytes[..model_row(15)]);
}

#[test]
fn aces_writes_exactly_its_row_count_and_fits_the_block() {
    for (what, settings) in [
        ("ACES по умолчанию", AcesSettings::default()),
        (
            "ACES с мягким сжатием входа",
            AcesSettings {
                input_gamut: WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
                ..AcesSettings::default()
            },
        ),
    ] {
        let (_, aces) = aces_params(settings);
        let mut rows = ToneBaseRows::default();
        aces::pack(&mut rows, &aces);
        assert_eq!(rows.len(), aces::ROW_COUNT, "{what}");
        assert_eq!(
            aces::ROW_COUNT,
            26 + 2 * aces::SPLINE_ROW_COUNT,
            "{what}: таблица строк"
        );
        assert!(FIRST_MODEL_ROW + rows.len() <= TONE_BASE_ROWS_MAX, "{what}");
    }
}
