//! Тесты отчёта о статистике кадра: отчёт есть только при потребителе, ожидание публикаций судит по периоду.

use render_loop::viewfinder::StatisticsDiagnostics;

use super::*;
use crate::scenario::camera::viewfinder_metrics::IspCountersDelta;

fn window(processed: u64, published: u64) -> WindowDelta {
    WindowDelta {
        window_ms: 60_000,
        isp: IspCountersDelta {
            frames_processed: processed,
            statistics_published: published,
            ..IspCountersDelta::default()
        },
        ..WindowDelta::default()
    }
}

#[test]
fn the_report_exists_only_when_the_render_has_a_consumer_of_statistics() {
    assert_eq!(
        FrameStatisticsReport::of_schedule(None),
        None,
        "нет потребителя — нет отчёта"
    );
    let schedule = StatisticsDiagnostics {
        every_n_frames: 6,
        requested: 10,
        skipped_by_period: 50,
    };
    assert_eq!(
        FrameStatisticsReport::of_schedule(Some(schedule)),
        Some(FrameStatisticsReport { every_n_frames: 6 })
    );
}

#[test]
fn a_healthy_decimated_window_passes_and_a_stalled_one_fails_with_the_expected_count() {
    let report = FrameStatisticsReport::four_for_tests();
    // 3600 кадров, каждый 4-й → 900; граница доли 0,5 (450) проходит, ниже — провал с числами.
    assert!(publication_violations(Some(&report), &window(3_600, 900), 0.5).is_empty());
    assert!(publication_violations(Some(&report), &window(3_600, 450), 0.5).is_empty());
    let violations = publication_violations(Some(&report), &window(3_600, 100), 0.5);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("опубликована 100 раз, ожидалось не меньше 450"),
        "{violations:?}"
    );
}

#[test]
fn extra_publications_of_the_ae_cold_start_are_not_a_failure() {
    let report = FrameStatisticsReport::four_for_tests();
    assert!(publication_violations(Some(&report), &window(3_600, 900 + 30), 0.5).is_empty());
    assert!(publication_violations(Some(&report), &window(3_600, 1_800), 0.5).is_empty());
}

#[test]
fn without_a_consumer_no_publications_are_expected_and_zero_is_not_red() {
    // Вендорский AE: статистика не считается, публикаций 0 — это норма, а не поломка.
    assert!(publication_violations(None, &window(3_600, 0), 0.5).is_empty());
}
