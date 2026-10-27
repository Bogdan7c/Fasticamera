use super::*;

#[test]
fn probe_is_disarmed_by_default() {
    let probe = CpuAccessProbe::default();
    assert_eq!(probe.mode(), CpuProbeMode::Disarmed);
    assert!(!probe.is_armed());
}

#[test]
fn arming_and_disarming_switch_the_mode_back_and_forth() {
    let probe = CpuAccessProbe::disarmed();
    probe.arm_copy_first_row();
    assert_eq!(probe.mode(), CpuProbeMode::CopyFirstRow);
    assert!(probe.is_armed());
    probe.disarm();
    assert!(!probe.is_armed());
}

#[test]
fn clones_share_one_switch_so_the_app_can_arm_what_the_listener_reads() {
    let app_handle = CpuAccessProbe::default();
    let listener_handle = app_handle.clone();
    app_handle.arm_copy_first_row();
    assert!(listener_handle.is_armed());
    listener_handle.disarm();
    assert!(!app_handle.is_armed());
}

#[test]
fn unknown_raw_state_counts_as_disarmed() {
    assert_eq!(CpuProbeMode::from_raw(200), CpuProbeMode::Disarmed);
}

#[test]
fn raw10_stride_is_bytes_and_raw16_stride_is_pixels() {
    assert_eq!(first_row_bytes(RawFormat::Raw10, 5120, usize::MAX), 5120);
    assert_eq!(first_row_bytes(RawFormat::Raw16, 4096, usize::MAX), 8192);
}

#[test]
fn first_row_never_exceeds_the_buffer_limit() {
    assert_eq!(first_row_bytes(RawFormat::Raw16, 4096, 1000), 1000);
    assert_eq!(first_row_bytes(RawFormat::Raw10, 0, 1000), 0);
}
