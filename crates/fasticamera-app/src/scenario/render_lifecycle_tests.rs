//! Вердикт `render-lifecycle`: проходной набор и каждое правило отказа.

use super::*;

/// Перепись телефона в покое: 3 fence, 5 картинок (виды с нулём не перечисляются).
fn idle_census() -> LiveObjectCensus {
    LiveObjectCensus::from([("Fence".to_owned(), 3), ("Image".to_owned(), 5)])
}

fn cycle(number: u32, kind: CycleKind, latency_ms: u64, presenting: u64) -> CycleRecord {
    CycleRecord {
        cycle: number,
        kind,
        release_latency_ms: Some(latency_ms),
        presenting_live_objects: Some(presenting),
        idle_live_objects: Some(idle_census()),
    }
}

fn clean_evidence() -> LifecycleEvidence {
    LifecycleEvidence {
        planned_cycles: 3,
        idle_baseline_live_objects: Some(idle_census()),
        cycles: vec![
            cycle(1, CycleKind::Home, 12, 21),
            cycle(2, CycleKind::Home, 30, 20),
            cycle(3, CycleKind::Screen, 18, 21),
        ],
        releases_confirmed: 3,
        surface_release_timeouts: 0,
        activity_recreated: false,
        validation: ValidationReportPart {
            mode: ValidationState::Enabled,
            errors: 0,
            warnings: 0,
            known_deviations: Default::default(),
        },
        fault: None,
        backend_errors: 0,
        detach_failures: 0,
    }
}

#[test]
fn clean_run_passes_and_reports_latency_max_and_median() {
    let verdict = evaluate(&clean_evidence());
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(verdict.report["release_latency"]["max_ms"], 30.0);
    assert_eq!(verdict.report["release_latency"]["median_ms"], 18.0);
    assert_eq!(verdict.report["idle_baseline_live_objects"]["Fence"], 3);
    assert_eq!(verdict.report["cycles"][1]["idle_live_objects"]["Image"], 5);
    assert_eq!(verdict.report["cycles"][1]["presenting_live_objects"], 20);
    assert_eq!(verdict.report["cycles"][2]["kind"], "screen");
    assert_eq!(verdict.report["releases_confirmed"], 3);
}

#[test]
fn fluctuating_presenting_counts_do_not_fail_when_idle_census_is_stable() {
    // Факт телефона: при показе 20/21 объектов (пул fence'ов показа), в покое всегда одно и то же.
    let mut evidence = clean_evidence();
    evidence.cycles[0].presenting_live_objects = Some(21);
    evidence.cycles[1].presenting_live_objects = Some(20);
    evidence.cycles[2].presenting_live_objects = Some(37);
    let verdict = evaluate(&evidence);
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(
        verdict.report["cycles"][2]["presenting_live_objects"], 37,
        "число остаётся в отчёте"
    );
}

#[test]
fn a_leaked_fence_in_the_idle_census_fails_naming_the_kind_and_the_cycle() {
    let mut evidence = clean_evidence();
    for record in &mut evidence.cycles[1..] {
        record
            .idle_live_objects
            .as_mut()
            .expect("перепись")
            .insert("Fence".to_owned(), 4);
    }
    let violations = evaluate(&evidence).violations;
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(violations[0].contains("цикл 2"), "{violations:?}");
    assert!(violations[0].contains("Fence: было 3, стало 4"), "{violations:?}");
    assert!(
        !violations[0].contains("Image"),
        "виды без изменений не называются: {violations:?}"
    );
}

#[test]
fn equal_totals_with_a_swapped_kind_still_fail() {
    // Сумма та же (8), но один Image заменился на Fence: сравнение обязано быть по видам.
    let mut evidence = clean_evidence();
    let swapped = LiveObjectCensus::from([("Fence".to_owned(), 4), ("Image".to_owned(), 4)]);
    evidence.cycles[1].idle_live_objects = Some(swapped);
    let violations = evaluate(&evidence).violations;
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("Fence: было 3, стало 4"), "{violations:?}");
    assert!(violations[0].contains("Image: было 5, стало 4"), "{violations:?}");
}

#[test]
fn a_kind_that_disappears_from_the_idle_census_fails_too() {
    let mut evidence = clean_evidence();
    evidence.cycles[2].idle_live_objects = Some(LiveObjectCensus::from([("Image".to_owned(), 5)]));
    let violations = evaluate(&evidence).violations;
    assert!(
        violations.iter().any(|text| text.contains("Fence: было 3, стало 0")),
        "{violations:?}"
    );
}

#[test]
fn unmeasured_cycle_and_missing_baseline_fail() {
    let mut evidence = clean_evidence();
    evidence.cycles[2].idle_live_objects = None;
    assert!(
        evaluate(&evidence)
            .violations
            .iter()
            .any(|text| text.contains("в покое не замерены"))
    );

    let mut no_baseline = clean_evidence();
    no_baseline.idle_baseline_live_objects = None;
    assert!(
        evaluate(&no_baseline)
            .violations
            .iter()
            .any(|text| text.contains("базовая перепись"))
    );
}

#[test]
fn release_count_must_equal_cycle_count() {
    let mut evidence = clean_evidence();
    evidence.releases_confirmed = 2;
    let verdict = evaluate(&evidence);
    assert!(
        verdict
            .violations
            .iter()
            .any(|text| text.contains("подтвердил освобождений: 2"))
    );
    evidence.releases_confirmed = 4;
    assert!(
        !evaluate(&evidence).violations.is_empty(),
        "лишнее освобождение тоже подозрительно"
    );
}

#[test]
fn incomplete_run_fails() {
    let mut evidence = clean_evidence();
    evidence.cycles.pop();
    assert!(
        evaluate(&evidence)
            .violations
            .iter()
            .any(|text| text.contains("2 из 3"))
    );
}

#[test]
fn shell_timeouts_activity_recreation_and_backend_problems_fail() {
    let mut evidence = clean_evidence();
    evidence.surface_release_timeouts = 1;
    evidence.activity_recreated = true;
    evidence.backend_errors = 1;
    evidence.detach_failures = 2;
    evidence.fault = Some("BackendPanic: x".to_owned());
    let violations = evaluate(&evidence).violations;
    for expected in [
        "таймауты освобождения",
        "Activity пересоздавалась",
        "ошибки бэкенда",
        "отсоединение",
        "неисправен",
    ] {
        assert!(
            violations.iter().any(|text| text.contains(expected)),
            "{expected}: {violations:?}"
        );
    }
}

#[test]
fn validation_errors_fail_only_when_enabled() {
    let mut evidence = clean_evidence();
    evidence.validation.errors = 5;
    assert!(
        evaluate(&evidence)
            .violations
            .iter()
            .any(|text| text.contains("validation"))
    );
    evidence.validation.mode = ValidationState::Disabled;
    assert_eq!(evaluate(&evidence).violations, Vec::<String>::new());
}
