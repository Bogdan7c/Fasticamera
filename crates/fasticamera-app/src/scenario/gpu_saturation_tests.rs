//! Мелкие правила сценария `gpu-saturation-bench` без маршрутизатора: чтение счётчиков рендера и итог наблюдения.
//! Сценарий целиком (стадии, камера, отмена, бюджет строки) — в `event_router_saturation_tests.rs`.

use super::*;
use crate::gpu_test_support::presenting_status;

#[test]
fn render_counters_come_from_the_existing_status_and_missing_viewfinder_means_zero_product_work() {
    let status = presenting_status();
    let activity = activity_of(&status);
    assert_eq!(activity.frames_presented, 1200, "показы — из статуса рендера");
    assert_eq!(
        activity.product_gpu_frames(),
        0,
        "видоискателя нет — продуктовой работы нет"
    );
}

#[test]
fn silence_is_observed_only_with_a_before_snapshot_otherwise_it_says_so() {
    let before = RenderActivity {
        isp_frames: 10,
        ..RenderActivity::default()
    };
    let after = RenderView::Live(RenderActivity {
        isp_frames: 13,
        frames_presented: 5,
        ..RenderActivity::default()
    });
    let RenderSilence::Observed { delta, .. } = render_silence(Some(before), &after) else {
        panic!("рендер жив, снимок «до» есть");
    };
    assert_eq!((delta.isp_frames, delta.frames_presented), (3, 5));
    assert!(matches!(
        render_silence(None, &after),
        RenderSilence::RenderUnavailable { .. }
    ));
    assert!(matches!(
        render_silence(Some(before), &RenderView::Unavailable("нет vsync".to_owned())),
        RenderSilence::RenderUnavailable { reason } if reason == "нет vsync"
    ));
}
