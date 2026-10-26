//! Вердикт `ahb-import` и сценарий на подставной диагностике.

use super::*;

fn summary() -> AhbCheckSummary {
    AhbCheckSummary {
        matched: true,
        second_import_hit: true,
        live_objects_restored: true,
        validation_errors: 0,
        known_deviations: Default::default(),
        validation: ValidationState::Enabled,
        details: "Raw16 4096x3072 matched=true".to_owned(),
    }
}

fn completed(change: impl FnOnce(&mut AhbCheckSummary)) -> AhbCheckResult {
    let mut value = summary();
    change(&mut value);
    AhbCheckResult::Completed(value)
}

fn unsupported() -> AhbCheckResult {
    AhbCheckResult::AllocationUnsupported {
        status: -22,
        requested: "AHardwareBufferDesc { format: 0x25 }".to_owned(),
    }
}

#[test]
fn both_formats_matched_pass() {
    let verdict = evaluate(4096, 3072, &completed(|_| {}), &completed(|_| {}));
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(verdict.report["raw16"]["outcome"], "matched");
    assert_eq!(verdict.report["raw10"]["outcome"], "matched");
    assert_eq!(verdict.report["width"], 4096);
}

#[test]
fn raw10_unsupported_is_reported_honestly_and_not_a_failure() {
    let verdict = evaluate(4096, 3072, &completed(|_| {}), &unsupported());
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(verdict.report["raw10"]["outcome"], "unsupported");
    assert!(
        verdict.report["raw10"]["details"]
            .as_str()
            .expect("текст")
            .contains("-22")
    );
}

#[test]
fn raw16_unsupported_is_a_failure() {
    let verdict = evaluate(4096, 3072, &unsupported(), &completed(|_| {}));
    assert!(
        verdict
            .violations
            .iter()
            .any(|text| text.contains("Raw16") && text.contains("не выделил"))
    );
}

#[test]
fn checksum_mismatch_cache_miss_leak_and_validation_each_fail() {
    for (name, change) in [
        (
            "суммы",
            Box::new(|s: &mut AhbCheckSummary| s.matched = false) as Box<dyn Fn(&mut AhbCheckSummary)>,
        ),
        ("кэш", Box::new(|s: &mut AhbCheckSummary| s.second_import_hit = false)),
        (
            "объекты",
            Box::new(|s: &mut AhbCheckSummary| s.live_objects_restored = false),
        ),
    ] {
        let verdict = evaluate(4096, 3072, &completed(&change), &completed(|_| {}));
        assert_eq!(verdict.violations.len(), 1, "{name}: {:?}", verdict.violations);
        assert_eq!(verdict.report["raw16"]["outcome"], "mismatch", "{name}");
        // Тот же дефект в RAW10 — тоже провал, а не «unsupported».
        let raw10 = evaluate(4096, 3072, &completed(|_| {}), &completed(&change));
        assert_eq!(raw10.violations.len(), 1, "{name}: {:?}", raw10.violations);
        assert!(raw10.violations[0].contains("Raw10"), "{name}");
    }
}

#[test]
fn matched_data_with_validation_errors_is_its_own_outcome_and_still_a_failure() {
    let with_errors = completed(|s| s.validation_errors = 2);
    let verdict = evaluate(4096, 3072, &with_errors, &completed(|_| {}));
    assert_eq!(verdict.report["raw16"]["outcome"], "matched_with_validation_errors");
    assert_eq!(verdict.violations.len(), 1, "{:?}", verdict.violations);
    assert!(verdict.violations[0].contains("Raw16") && verdict.violations[0].contains("validation"));
    // Тот же дефект в RAW10 — тоже провал.
    let raw10 = evaluate(4096, 3072, &completed(|_| {}), &with_errors);
    assert_eq!(raw10.report["raw10"]["outcome"], "matched_with_validation_errors");
    assert_eq!(raw10.violations.len(), 1);
}

#[test]
fn checksum_mismatch_wins_over_validation_errors() {
    let both = completed(|s| {
        s.matched = false;
        s.validation_errors = 5;
    });
    let verdict = evaluate(4096, 3072, &both, &completed(|_| {}));
    assert_eq!(verdict.report["raw16"]["outcome"], "mismatch");
    assert_eq!(verdict.violations.len(), 2, "{:?}", verdict.violations);
}

#[test]
fn known_deviations_pass_and_are_reported_per_vuid() {
    let with_known = completed(|s| {
        s.known_deviations = [
            ("VUID-VkMemoryAllocateInfo-pNext-02387".to_owned(), 1),
            ("VUID-VkMemoryAllocateInfo-pNext-02384".to_owned(), 0),
        ]
        .into();
    });
    let verdict = evaluate(4096, 3072, &with_known, &completed(|_| {}));
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(verdict.report["raw16"]["outcome"], "matched");
    let reported = &verdict.report["raw16"]["summary"]["known_deviations"];
    assert_eq!(reported["VUID-VkMemoryAllocateInfo-pNext-02387"], 1);
    assert_eq!(reported["VUID-VkMemoryAllocateInfo-pNext-02384"], 0);
}

#[test]
fn validation_errors_are_ignored_when_layer_is_disabled() {
    let release = completed(|s| {
        s.validation = ValidationState::Disabled;
        s.validation_errors = 0;
    });
    assert_eq!(
        evaluate(4096, 3072, &release, &release).violations,
        Vec::<String>::new()
    );
}

#[test]
fn diagnostic_error_fails_with_reason() {
    let broken = AhbCheckResult::Failed {
        reason: "GPU не поднялся: нет Vulkan".to_owned(),
    };
    let verdict = evaluate(4096, 3072, &broken, &completed(|_| {}));
    assert!(verdict.violations.iter().any(|text| text.contains("нет Vulkan")));
    assert_eq!(verdict.report["raw16"]["outcome"], "error");
}
