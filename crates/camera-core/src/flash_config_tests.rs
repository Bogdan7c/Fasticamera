//! Тесты конфига вспышки: умолчания проходят, каждая граница ловится своим именем поля,
//! худший срок последовательности — сумма её частей.

use super::*;

/// Портит одно поле конфига.
type Spoil = fn(&mut FlashConfig);

#[test]
fn defaults_are_valid_and_carry_the_measured_ratio() {
    let config = FlashConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(config.torch_to_flash_ratio, 1.87);
    assert_eq!(config.torch_off_to_single_gap_ms, 100);
}

#[test]
fn every_out_of_range_field_is_named_in_the_error() {
    let cases: [(&str, Spoil); 9] = [
        ("torch_to_flash_ratio", |c| c.torch_to_flash_ratio = 0.5),
        ("auto_fire_min_mid_gray_log2_seconds_iso", |c| {
            c.auto_fire_min_mid_gray_log2_seconds_iso = f64::NAN
        }),
        ("preflash_max_log2_seconds_iso", |c| {
            c.preflash_max_log2_seconds_iso = 50.0
        }),
        ("preflash_remeasure_step_ev", |c| c.preflash_remeasure_step_ev = 0.0),
        ("max_preflash_remeasures", |c| c.max_preflash_remeasures = 4),
        ("preflash_timeout_ms", |c| c.preflash_timeout_ms = 99),
        // Факт 14d: пауза короче 50 мс не проверена — конфиг её не пускает.
        ("torch_off_to_single_gap_ms", |c| c.torch_off_to_single_gap_ms = 49),
        ("ambient_sample_max_age_ms", |c| c.ambient_sample_max_age_ms = 99),
        ("result_timeout_ms", |c| c.result_timeout_ms = 5_001),
    ];
    for (field, spoil) in cases {
        let mut config = FlashConfig::default();
        spoil(&mut config);
        match config.validate() {
            Err(FlashConfigError::OutOfRange { field: named, .. }) => assert_eq!(named, field),
            Ok(()) => panic!("{field}: испорченное значение прошло проверку"),
        }
    }
}

#[test]
fn worst_case_sequence_adds_preflash_gap_and_result() {
    let config = FlashConfig::default();
    // Один перемер по умолчанию: предвспышка длится до двух сроков (срок + продление), см. `FlashConfig`.
    assert_eq!(config.worst_case_sequence_ms(), 500 * 2 + 100 + 400);
}

#[test]
fn worst_case_sequence_grows_by_one_preflash_timeout_per_remeasure() {
    let without_remeasures = FlashConfig {
        max_preflash_remeasures: 0,
        ..FlashConfig::default()
    };
    let three_remeasures = FlashConfig {
        max_preflash_remeasures: 3,
        ..FlashConfig::default()
    };
    assert_eq!(without_remeasures.worst_case_sequence_ms(), 500 + 100 + 400);
    assert_eq!(three_remeasures.worst_case_sequence_ms(), 500 * 4 + 100 + 400);
}
