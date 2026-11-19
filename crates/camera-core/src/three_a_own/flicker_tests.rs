//! Ручная защита от мерцания: период кратности по политике и частоте сети (решение 77).

use std::time::Duration;

use camera_contract::three_a::MainsFrequency;

use super::config::FlickerPolicy;
use super::flicker::{FlickerProtection, half_mains_period};

#[test]
fn half_period_is_10ms_at_50hz_and_8_33ms_at_60hz() {
    assert_eq!(half_mains_period(MainsFrequency::Hz50), Duration::from_millis(10));
    assert_eq!(half_mains_period(MainsFrequency::Hz60), Duration::from_nanos(8_333_333));
}

#[test]
fn always_protect_gives_the_half_period_of_the_configured_mains() {
    let at_50 = FlickerProtection::new(FlickerPolicy::AlwaysProtect, MainsFrequency::Hz50);
    let at_60 = FlickerProtection::new(FlickerPolicy::AlwaysProtect, MainsFrequency::Hz60);
    assert_eq!(at_50.period(), Some(Duration::from_millis(10)));
    assert_eq!(at_60.period(), Some(Duration::from_nanos(8_333_333)));
}

#[test]
fn never_protect_gives_no_period_for_any_mains() {
    for mains in [MainsFrequency::Hz50, MainsFrequency::Hz60] {
        assert_eq!(
            FlickerProtection::new(FlickerPolicy::NeverProtect, mains).period(),
            None
        );
    }
}

#[test]
fn default_policy_is_never_protect() {
    assert_eq!(FlickerPolicy::default(), FlickerPolicy::NeverProtect);
}
