//! Значения и проверки слежения.

use std::time::Duration;

use camera_contract::white::WhiteTransition;

use super::{WhiteFollowConfig, WhiteFollowSpeed};
use crate::white::{WhiteConfig, WhiteConfigError};

#[test]
fn the_speed_is_a_transition_and_a_tenth_of_a_second_is_allowed() {
    let speed = WhiteFollowSpeed::from_millis(100).expect("0,1 с разрешена (WB56)");
    assert_eq!(speed.transition(), WhiteTransition::Over(Duration::from_millis(100)));
    let at_once = WhiteFollowSpeed::from_millis(0).expect("ноль — сразу");
    assert_eq!(at_once.transition(), WhiteTransition::Immediate);
}

#[test]
fn a_speed_beyond_the_limit_is_refused_by_value_and_by_json() {
    let refused = WhiteFollowSpeed::from_millis(10_001).expect_err("больше предела");
    assert_eq!(refused.requested_ms, 10_001);
    assert_eq!(
        WhiteFollowSpeed::from_millis(10_000).map(WhiteFollowSpeed::millis),
        Ok(10_000)
    );

    let parsed: WhiteFollowSpeed = serde_json::from_str("100").expect("число миллисекунд");
    assert_eq!(parsed.millis(), 100);
    assert_eq!(serde_json::to_string(&parsed).expect("запись"), "100");
    let error = serde_json::from_str::<WhiteFollowSpeed>("10001").expect_err("больше предела");
    assert!(error.to_string().contains("10001"), "{error}");
}

/// Портит одно число секции.
type Spoil = fn(&mut WhiteFollowConfig);

#[test]
fn every_bad_number_of_the_section_is_refused_with_its_field() {
    let cases: [(&str, Spoil); 7] = [
        ("follow.ask_interval_ms", |config| config.ask_interval_ms = 0),
        ("follow.ask_interval_ms", |config| config.ask_interval_ms = 60_001),
        ("follow.transition_ms", |config| config.transition_ms = 10_001),
        ("follow.settle_after_flash_ms", |config| {
            config.settle_after_flash_ms = 60_001
        }),
        ("follow.dead_zone_mired", |config| config.dead_zone_mired = f64::NAN),
        ("follow.dead_zone_mired", |config| config.dead_zone_mired = -0.1),
        ("follow.dead_zone_duv", |config| config.dead_zone_duv = f64::INFINITY),
    ];
    for (expected_field, spoil) in cases {
        // Через общий конфиг белого: вложенная секция обязана проверяться вместе с ним.
        let mut config = WhiteConfig::default();
        spoil(&mut config.follow);
        match config.validate() {
            Err(WhiteConfigError::Invalid { field, .. }) => assert_eq!(field, expected_field),
            Ok(()) => panic!("{expected_field}: ошибка пропущена"),
        }
    }
    assert_eq!(WhiteConfig::default().validate(), Ok(()));
}

#[test]
fn the_default_speed_is_the_owner_choice_and_follows_the_config() {
    assert_eq!(WhiteFollowConfig::default().default_speed().millis(), 300);
    let config = WhiteFollowConfig {
        transition_ms: 100,
        ..WhiteFollowConfig::default()
    };
    assert_eq!(config.default_speed().millis(), 100);
}
