use std::sync::Arc;

use raw_frame_stream::{RawFrameStream, RawFrameStreamConfig};

use super::*;
use crate::raw_stream_services::RawStreamServices;

/// Сервис над НАСТОЯЩИМ потоком кадров: переключение видно в самом потоке, а не в подставке.
fn services_over_real_stream(initial: ViewfinderPacing) -> (RawStreamServices, RawFrameStream) {
    let config = RawFrameStreamConfig {
        viewfinder_pacing: initial,
        ..RawFrameStreamConfig::default()
    };
    let stream = RawFrameStream::new(config.clone());
    let services = RawStreamServices::detached_with(config).with_pacing_control(Arc::new(stream.clone()));
    (services, stream)
}

#[test]
fn local_pacing_reports_switched_then_already_active() {
    let local = LocalPacing::new(ViewfinderPacing::LatestWins);

    assert_eq!(
        local.switch_to(ViewfinderPacing::LatestWins),
        ViewfinderPacingSwitch::AlreadyActive
    );
    assert_eq!(
        local.switch_to(ViewfinderPacing::JitterBuffer),
        ViewfinderPacingSwitch::Switched
    );
    assert_eq!(local.pacing(), ViewfinderPacing::JitterBuffer);
}

#[test]
fn claim_keeps_the_first_original_and_gives_it_out_once() {
    let claim = PacingClaim::default();
    assert_eq!(claim.take_original(), None, "ничего не трогали");

    claim.remember_original(ViewfinderPacing::JitterBuffer);
    claim.remember_original(ViewfinderPacing::LatestWins);

    assert_eq!(claim.take_original(), Some(ViewfinderPacing::JitterBuffer));
    assert_eq!(claim.take_original(), None);
}

#[test]
fn scenario_switch_reaches_the_real_stream_and_restore_brings_the_original_back() {
    let (services, stream) = services_over_real_stream(ViewfinderPacing::JitterBuffer);
    let switches_before = stream.counters().viewfinder_pacing_switches;

    // Два окна подряд: LatestWins, затем джиттер-буфер, затем снова LatestWins.
    assert_eq!(
        services.set_viewfinder_pacing_for_scenario(ViewfinderPacing::LatestWins),
        ViewfinderPacingSwitch::Switched
    );
    assert_eq!(stream.viewfinder_pacing(), ViewfinderPacing::LatestWins);
    services.set_viewfinder_pacing_for_scenario(ViewfinderPacing::JitterBuffer);
    services.set_viewfinder_pacing_for_scenario(ViewfinderPacing::LatestWins);

    // Возврат к тому, что было ДО сценария (джиттер-буфер), а не к последнему окну.
    assert_eq!(
        services.restore_viewfinder_pacing_after_scenario(),
        Some(ViewfinderPacingSwitch::Switched)
    );
    assert_eq!(stream.viewfinder_pacing(), ViewfinderPacing::JitterBuffer);
    assert_eq!(stream.counters().viewfinder_pacing_switches - switches_before, 4);

    // Учёт забран: второй конец сценария ничего не трогает.
    assert_eq!(services.restore_viewfinder_pacing_after_scenario(), None);
    assert_eq!(stream.counters().viewfinder_pacing_switches - switches_before, 4);
}

#[test]
fn untouched_pacing_is_not_restored_and_the_stream_keeps_its_policy() {
    let (services, stream) = services_over_real_stream(ViewfinderPacing::JitterBuffer);

    assert_eq!(services.restore_viewfinder_pacing_after_scenario(), None);

    assert_eq!(stream.viewfinder_pacing(), ViewfinderPacing::JitterBuffer);
    assert_eq!(stream.counters().viewfinder_pacing_switches, 0);
}

#[test]
fn restore_after_a_no_op_switch_reports_already_active() {
    let (services, _stream) = services_over_real_stream(ViewfinderPacing::LatestWins);

    assert_eq!(
        services.set_viewfinder_pacing_for_scenario(ViewfinderPacing::LatestWins),
        ViewfinderPacingSwitch::AlreadyActive
    );

    assert_eq!(
        services.restore_viewfinder_pacing_after_scenario(),
        Some(ViewfinderPacingSwitch::AlreadyActive)
    );
}

#[test]
fn detached_services_start_from_the_configured_pacing() {
    let services = RawStreamServices::detached();

    assert_eq!(
        services.viewfinder_pacing(),
        RawFrameStreamConfig::default().viewfinder_pacing
    );
}
