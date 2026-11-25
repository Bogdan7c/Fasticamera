//! Тесты конфига вспышки: умолчания проходят, каждая граница ловится своим именем поля,
//! худший срок последовательности — сумма её частей.

use camera_contract::ids::LensRole;

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

#[test]
fn led_white_defaults_are_the_measured_phone_numbers_and_other_follows_the_main_camera() {
    let led = FlashConfig::default().led_white;
    assert_eq!(
        led.white_for(Some(LensRole::Wide)),
        LedWhite {
            kelvin: 3_980.0,
            duv: 0.0068
        }
    );
    assert_eq!(
        led.white_for(Some(LensRole::UltraWide)),
        LedWhite {
            kelvin: 3_815.0,
            duv: 0.0035
        }
    );
    for other in [None, Some(LensRole::Tele), Some(LensRole::Front)] {
        assert_eq!(led.white_for(other), led.wide, "роль {other:?}");
    }
}

#[test]
fn white_for_picks_the_field_of_the_role_and_other_for_the_rest() {
    let led = FlashLedWhite {
        wide: LedWhite {
            kelvin: 3_000.0,
            duv: 0.0,
        },
        ultra_wide: LedWhite {
            kelvin: 5_000.0,
            duv: 0.0,
        },
        other: LedWhite {
            kelvin: 7_000.0,
            duv: 0.0,
        },
    };
    assert_eq!(led.white_for(Some(LensRole::Wide)).kelvin, 3_000.0);
    assert_eq!(led.white_for(Some(LensRole::UltraWide)).kelvin, 5_000.0);
    for other in [Some(LensRole::Tele), Some(LensRole::Front), None] {
        assert_eq!(led.white_for(other).kelvin, 7_000.0, "{other:?}");
    }
}

#[test]
fn every_bad_led_white_field_is_named_with_its_full_path() {
    let cases: [(&str, Spoil); 6] = [
        ("led_white.wide.kelvin", |c| c.led_white.wide.kelvin = 1_000.0),
        ("led_white.wide.duv", |c| c.led_white.wide.duv = 0.06),
        ("led_white.ultra_wide.kelvin", |c| {
            c.led_white.ultra_wide.kelvin = 25_000.0
        }),
        ("led_white.ultra_wide.duv", |c| c.led_white.ultra_wide.duv = f64::NAN),
        ("led_white.other.kelvin", |c| c.led_white.other.kelvin = f64::INFINITY),
        ("led_white.other.duv", |c| c.led_white.other.duv = -0.051),
    ];
    for (field, spoil) in cases {
        let mut config = FlashConfig::default();
        spoil(&mut config);
        match config.validate() {
            Err(error @ FlashConfigError::OutOfRange { field: named, .. }) => {
                assert_eq!(named, field);
                assert!(error.to_string().contains(&format!("three_a.flash.{field}")), "{error}");
            }
            Ok(()) => panic!("{field}: испорченное значение прошло проверку"),
        }
    }
}

#[test]
fn led_white_range_edges_are_accepted() {
    let mut config = FlashConfig::default();
    config.led_white.wide = LedWhite {
        kelvin: 1_667.0,
        duv: -0.05,
    };
    config.led_white.other = LedWhite {
        kelvin: 20_000.0,
        duv: 0.05,
    };
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn led_white_converts_to_a_contract_setting_and_unknown_json_fields_are_rejected() {
    let setting = LedWhite {
        kelvin: 4_200.0,
        duv: 0.004,
    }
    .to_setting()
    .expect("годная белая");
    assert_eq!((setting.temperature.kelvin(), setting.tint.duv()), (4_200.0, 0.004));
    let parsed: FlashConfig =
        serde_json::from_str(r#"{"led_white":{"ultra_wide":{"kelvin":5200.0}}}"#).expect("частичный конфиг читается");
    assert_eq!(
        parsed.led_white.ultra_wide,
        LedWhite {
            kelvin: 5_200.0,
            duv: 0.0
        }
    );
    assert_eq!(
        parsed.led_white.wide,
        FlashLedWhite::default().wide,
        "не названное — по умолчанию"
    );
    assert!(serde_json::from_str::<FlashConfig>(r#"{"led_white":{"tele":{}}}"#).is_err());
}
