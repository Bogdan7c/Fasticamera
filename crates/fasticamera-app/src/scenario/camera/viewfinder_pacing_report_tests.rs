use super::*;

fn counters(change: impl FnOnce(&mut RawStreamCounters)) -> RawStreamCounters {
    let mut counters = RawStreamCounters::default();
    change(&mut counters);
    counters
}

#[test]
fn jitter_buffer_window_counts_overflow_as_drop_and_repeats_from_the_display() {
    // 10 с при 60 к/с: 600 опубликовано, 598 выдано, 1 переполнение, 1 осталось в очереди.
    let stream = counters(|c| {
        c.frames_published = 600;
        c.viewfinder_delivered = 598;
        c.dropped_jitter_overflow = 1;
        c.viewfinder_queued = 1;
        c.viewfinder_underruns = 2;
        c.viewfinder_polls_while_priming = 0;
    });

    let report = PacingWindowReport::of_window(&stream, 2, 10_000);

    assert_eq!(report.dropped, 1);
    assert_eq!(report.superseded, 0);
    assert_eq!(report.jitter_overflow, 1);
    assert_eq!(report.repeats, 2);
    assert_eq!(report.underruns, 2);
    assert_eq!(report.shown_once_ratio, Some(0.9967));
    assert_eq!(report.dropped_per_s, Some(0.1));
    assert_eq!(report.repeats_per_s, Some(0.2));
}

#[test]
fn latest_wins_window_sums_superseded_and_overflow_into_drops() {
    let stream = counters(|c| {
        c.frames_published = 600;
        c.viewfinder_delivered = 540;
        c.dropped_superseded = 59;
        c.dropped_jitter_overflow = 1;
    });

    let report = PacingWindowReport::of_window(&stream, 60, 10_000);

    assert_eq!(report.dropped, 60);
    assert_eq!(report.superseded, 59);
    assert_eq!(report.shown_once_ratio, Some(0.9));
    assert_eq!(report.repeats_per_s, Some(6.0));
}

#[test]
fn empty_window_has_no_ratios_instead_of_division_by_zero() {
    let report = PacingWindowReport::of_window(&RawStreamCounters::default(), 0, 0);

    assert_eq!(report, PacingWindowReport::default());
    assert_eq!(report.shown_once_ratio, None);
    assert_eq!(report.dropped_per_s, None);
}
