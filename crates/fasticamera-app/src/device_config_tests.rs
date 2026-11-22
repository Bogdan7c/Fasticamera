#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use super::*;

#[test]
fn full_section_gives_trimmed_identity() {
    let config: DeviceConfig = serde_json::from_str(r#"{"make":" OnePlus ","model":"CPH2449"}"#).unwrap();
    let identity = config.identity().unwrap();
    assert_eq!((identity.make(), identity.model()), ("OnePlus", "CPH2449"));
}

#[test]
fn absent_section_is_explicit_absence_not_placeholder() {
    let config = DeviceConfig::default();
    assert_eq!(config.identity(), Err(DeviceIdentityError::Absent { field: "make" }));
    let only_make = DeviceConfig {
        make: Some("OnePlus".to_owned()),
        model: None,
    };
    assert_eq!(
        only_make.identity(),
        Err(DeviceIdentityError::Absent { field: "model" })
    );
}

#[test]
fn blank_field_is_rejected_by_validate() {
    let config = DeviceConfig {
        make: Some("OnePlus".to_owned()),
        model: Some("   ".to_owned()),
    };
    assert_eq!(config.validate(), Err(DeviceIdentityError::Blank { field: "model" }));
}

#[test]
fn unknown_field_is_a_config_error() {
    assert!(serde_json::from_str::<DeviceConfig>(r#"{"maker":"x"}"#).is_err());
}
