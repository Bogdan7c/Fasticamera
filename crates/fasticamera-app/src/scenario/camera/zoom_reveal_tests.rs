//! Приращения «плохих» исходов держания считаются от снимка начала окна, а не от нуля.

use render_loop::viewfinder::ContinuityCounters;

use super::*;

#[test]
fn faults_are_deltas_between_two_counter_snapshots() {
    let start = ContinuityCounters {
        holds_revealed_by_timeout: 2,
        holds_ended_without_stream: 1,
        black_frames_camera_unavailable: 5,
        ..ContinuityCounters::default()
    };
    let end = ContinuityCounters {
        holds_revealed_by_timeout: 3,
        holds_ended_without_stream: 1,
        holds_ended_without_presentable_frame: 4,
        black_frames_camera_unavailable: 5,
        black_frames_picture_unavailable: 7,
        ..ContinuityCounters::default()
    };
    let faults = RevealFaults::between(&start, &end);
    assert_eq!(
        faults,
        RevealFaults {
            revealed_by_timeout: 1,
            ended_without_stream: 0,
            ended_without_presentable_frame: 4,
            black_camera_unavailable: 0,
            black_picture_unavailable: 7,
        }
    );
    assert!(!faults.is_clean());
}

#[test]
fn identical_snapshots_are_clean_and_describe_nothing() {
    let counters = ContinuityCounters {
        holds_revealed_by_timeout: 9,
        ..ContinuityCounters::default()
    };
    let faults = RevealFaults::between(&counters, &counters);
    assert!(faults.is_clean());
    assert_eq!(faults.describe(), "");
}

#[test]
fn description_names_only_the_nonzero_faults() {
    let faults = RevealFaults {
        revealed_by_timeout: 2,
        black_picture_unavailable: 1,
        ..RevealFaults::default()
    };
    let text = faults.describe();
    assert!(text.contains("по сроку вместо условия готовности: 2"), "{text}");
    assert!(text.contains("«картинка недоступна»: 1"), "{text}");
    assert!(!text.contains("без потока"), "{text}");
}
