//! Счётчик «результатов с активным AE HAL» (08c): считается движком по каждому результату любого
//! происхождения, без выборки. Сценарий `3a` видит только последний результат на тик (примерно
//! каждый третий кадр), а критерий выхода 08a — «вендорский AE не включается ни в одном запросе».

use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::three_a::{AeState, BurstId, PointRequestId, RequestOrigin};

use super::three_a_support::*;

/// Результат кадра с заданным `AE_STATE`.
fn result_with_ae(frame: i64, origin: RequestOrigin, ae_state: Option<AeState>) -> CaptureResultMeta {
    let mut result = result_meta(frame, origin, None);
    result.three_a.ae_state = ae_state;
    result
}

#[test]
fn every_result_is_counted_and_only_active_hal_ae_counts_as_vendor() {
    let mut rig = streaming_rig();
    let before = rig.engine.status().three_a.counters;
    // Кадр 12 — разовый тап, кадр 13 — кадр серии: у обоих HAL сообщил активный AE. Затем более
    // новый кадр потока с `Inactive` перекрывает «последний результат» — читатель статуса тап и
    // серию уже не увидит, а счётчик движка обязан их посчитать.
    rig.feed_result(result_with_ae(10, RequestOrigin::Repeating, Some(AeState::Inactive)));
    rig.feed_result(result_with_ae(11, RequestOrigin::Repeating, None));
    rig.feed_result(result_with_ae(
        12,
        RequestOrigin::FocusTrigger {
            request: PointRequestId(1),
        },
        Some(AeState::Searching),
    ));
    rig.feed_result(result_with_ae(
        13,
        RequestOrigin::Burst {
            burst: BurstId(1),
            index: 0,
        },
        Some(AeState::Converged),
    ));
    rig.feed_result(result_with_ae(14, RequestOrigin::Repeating, Some(AeState::Inactive)));
    let counters = rig.engine.status().three_a.counters;
    assert_eq!(counters.results_observed - before.results_observed, 5);
    assert_eq!(
        counters.results_with_hal_ae_active - before.results_with_hal_ae_active,
        2
    );
    // «Последний результат» — кадр потока: по нему одному вендорский AE был бы не виден.
    let last = rig.engine.status().three_a.last_result.expect("результат есть");
    assert_eq!(last.ae_state, Some(AeState::Inactive));
}
