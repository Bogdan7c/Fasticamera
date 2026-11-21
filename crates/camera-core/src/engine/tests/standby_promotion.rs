//! Повышение тёплой до основной: смена объектива обменом ролей вместо закрытия и открытия.

use camera_contract::ids::LensRole;
use camera_contract::zoom::{StandbyDemand, ZoomGesture};

use super::standby_support::*;
use super::*;
use crate::backend::{BackendEvent, CaptureFailureReason, SessionGeneration};
use crate::backend_standby::StandbySwapFailure;
use crate::lens_status::{LensSwitchMethod, LensSwitchState};
use crate::standby_status::StandbySlotState;

fn switched_method(events: &[CameraEvent]) -> Vec<(LensRole, LensRole, LensSwitchMethod)> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::LensSwitched { from, to, method, .. } => Some((*from, *to, *method)),
            _ => None,
        })
        .collect()
}

fn warm_ready_rig() -> StandbyRig {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.wheel_at(1.0);
    rig.make_standby_ready(0);
    rig.lens.rig.fresh_events();
    rig.calls();
    rig
}

#[test]
fn ready_standby_is_promoted_without_close_or_open_and_switch_finishes_on_the_first_new_frame() {
    let mut rig = warm_ready_rig();
    let old_primary_session = rig.lens.rig.fake.current_session_generation().expect("сессия основной");
    let standby_session = rig
        .lens
        .rig
        .fake
        .standby_session_generation(slot(0))
        .expect("сессия тёплой");

    rig.tap_while_warm(0.5);

    let calls = rig.calls();
    assert!(calls.contains(&"swap_standby_with_primary"), "{calls:?}");
    for forbidden in ["close_device", "close_session", "open_device", "configure_session"] {
        assert!(
            !calls.contains(&forbidden),
            "{forbidden} при повышении быть не должно: {calls:?}"
        );
    }
    // Бывшая тёплая — основная, прежняя основная стала тёплой и готова.
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    assert_eq!(rig.lens.rig.fake.current_session_generation(), Some(standby_session));
    assert_eq!(rig.lens.rig.fake.standby_camera(slot(0)), Some(wide_id()));
    assert_eq!(
        rig.lens.rig.fake.standby_session_generation(slot(0)),
        Some(old_primary_session)
    );
    assert_eq!(rig.slot_state(0), StandbySlotState::Ready);
    assert_eq!(rig.standby().counters.promotions, 1);
    // Смена — то же состояние `Switching`, пока первого кадра новой основной нет.
    assert_eq!(
        rig.lens.switch_state(),
        LensSwitchState::Switching {
            from: LensRole::Wide,
            to: LensRole::UltraWide
        }
    );
    assert_eq!(rig.lens.rig.state_name(), "streaming", "поток не прерывался");
    assert!(switched_method(&rig.lens.rig.fresh_events()).is_empty());
    // Темп начался заново на сессии новой основной.
    let stream = rig.lens.rig.engine.status().stream;
    assert_eq!((stream.session, stream.results_completed), (Some(standby_session), 0));

    rig.lens.frames(1);
    assert_eq!(
        switched_method(&rig.lens.rig.fresh_events()),
        vec![(LensRole::Wide, LensRole::UltraWide, LensSwitchMethod::PromotedStandby)]
    );
    assert_eq!(
        rig.lens.switch_state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
    assert_eq!(rig.lens.rig.engine.status().stream.results_completed, 1);
}

#[test]
fn promotion_reissues_the_primary_request_and_re_requests_the_ex_primary_as_standby() {
    let mut rig = warm_ready_rig();
    rig.tap_while_warm(0.5);
    // Новая основная получает свой запрос по плану 3A, прежняя основная — запрос тёплой.
    assert_eq!(
        rig.calls(),
        vec![
            "swap_standby_with_primary",
            "set_repeating_request",
            "set_standby_repeating_request"
        ]
    );
    assert!(rig.lens.rig.fake.is_repeating());
    assert!(rig.lens.rig.fake.standby_repeating_request(slot(0)).is_some());
    assert_eq!(rig.lens.rig.state_name(), "streaming");
}

#[test]
fn standby_that_is_still_warming_is_not_promoted_the_switch_reopens() {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.wheel_at(1.0);
    assert_eq!(rig.slot_state(0), StandbySlotState::Warming);
    rig.calls();

    rig.tap_while_warm(0.5);

    let calls = rig.calls();
    assert!(!calls.contains(&"swap_standby_with_primary"), "{calls:?}");
    assert!(
        calls.contains(&"close_device") && calls.contains(&"open_device"),
        "{calls:?}"
    );
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    rig.lens.frames(1);
    assert_eq!(
        switched_method(&rig.lens.rig.fresh_events()),
        vec![(LensRole::Wide, LensRole::UltraWide, LensSwitchMethod::Reopened)]
    );
    assert_eq!(rig.standby().counters.promotions, 0);
}

#[test]
fn swap_back_while_demand_persists_promotes_the_ex_primary_again() {
    let mut rig = warm_ready_rig();
    rig.tap_while_warm(0.5);
    rig.lens.frames(1);
    rig.lens.rig.fresh_events();
    rig.calls();

    // Спрос всё ещё есть: прежняя основная теперь тёплая и готова — возвращаемся обменом.
    rig.tap_while_warm(1.0);

    let calls = rig.calls();
    assert!(calls.contains(&"swap_standby_with_primary"), "{calls:?}");
    assert!(
        !calls.contains(&"close_device") && !calls.contains(&"open_device"),
        "{calls:?}"
    );
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(wide_id()));
    assert_eq!(rig.standby().counters.promotions, 2);
    rig.lens.frames(1);
    assert_eq!(
        switched_method(&rig.lens.rig.fresh_events()),
        vec![(LensRole::UltraWide, LensRole::Wide, LensSwitchMethod::PromotedStandby)]
    );
}

#[test]
fn ex_primary_in_the_slot_stays_subject_to_release_after_promotion() {
    let mut rig = warm_ready_rig();
    rig.tap_while_warm(0.5);
    rig.lens.frames(1);
    rig.calls();
    rig.wheel_closed_at(0.5);
    rig.lens.rig.advance(Duration::from_millis(
        CameraEngineConfig::default().standby.release_delay_ms,
    ));
    assert_eq!(rig.calls(), vec!["close_standby"]);
    assert_eq!(
        rig.lens.rig.fake.open_camera(),
        Some(ultra_wide_id()),
        "новая основная жива"
    );
    assert_eq!(rig.slot_state(0), StandbySlotState::Idle);
}

#[test]
fn queued_results_of_the_old_primary_are_counted_separately_from_stale_events() {
    let mut rig = warm_ready_rig();
    let old_primary_session = rig.lens.rig.fake.current_session_generation().expect("сессия основной");
    rig.tap_while_warm(0.5);
    let before = rig.lens.rig.engine.status();

    // Результаты прежней основной, которые лежали в ящике в момент обмена.
    rig.lens
        .rig
        .fake
        .complete_frames_of_session(old_primary_session, 3, Duration::from_millis(16));
    rig.lens.rig.feed(BackendEvent::CaptureFailed {
        session: old_primary_session,
        frame_number: camera_contract::capture_result::FrameNumber(1),
        reason: CaptureFailureReason::Flushed,
    });

    let after = rig.lens.rig.engine.status();
    assert_eq!(after.lens.standby.counters.results_dropped_after_swap, 4);
    assert_eq!(
        after.counters.stale_backend_events,
        before.counters.stale_backend_events
    );
    assert_eq!(after.stream, before.stream, "темп новой основной не тронут");
    assert_eq!(after.three_a, before.three_a);
    assert_eq!(rig.lens.switch_state(), before.lens.switch, "и учёт смены не тронут");

    // Настоящее привидение (поколение, которого нет нигде) — по-прежнему устаревшее.
    rig.lens.rig.feed(BackendEvent::StandbyStreaming {
        session: SessionGeneration(9_999),
    });
    rig.lens
        .rig
        .fake
        .complete_frames_of_session(SessionGeneration(9_998), 1, Duration::from_millis(16));
    rig.lens.rig.pump();
    let last = rig.lens.rig.engine.status();
    assert_eq!(
        last.counters.stale_backend_events,
        before.counters.stale_backend_events + 2
    );
    assert_eq!(last.lens.standby.counters.results_dropped_after_swap, 4);
}

#[test]
fn a_standby_streaming_event_that_arrives_after_the_swap_is_not_counted_as_stale() {
    // Гонка обмена в бэкенде: колбэк NDK прочёл роль «тёплая», а событие «тёплая готова» доставил
    // уже после обмена — его сессия теперь основная. Инвариант 4: это не устаревшее событие.
    let mut rig = warm_ready_rig();
    rig.tap_while_warm(0.5);
    let new_primary_session = rig
        .lens
        .rig
        .fake
        .current_session_generation()
        .expect("сессия новой основной");
    let before = rig.lens.rig.engine.status();

    rig.lens.rig.feed(BackendEvent::StandbyStreaming {
        session: new_primary_session,
    });

    let after = rig.lens.rig.engine.status();
    assert_eq!(
        after.counters.stale_backend_events,
        before.counters.stale_backend_events
    );
    assert_eq!(
        after.lens.standby.counters.results_dropped_after_swap,
        before.lens.standby.counters.results_dropped_after_swap + 1
    );
    assert_eq!(after.lens.standby.slots, before.lens.standby.slots, "ячейки не тронуты");
    assert_eq!(after.state, before.state, "основная не тронута");
}

#[test]
fn backend_refusing_the_swap_falls_back_to_reopening_and_counts_it() {
    // Статус бэкенда «у основной нет запроса» нельзя вызвать без рассинхрона, поэтому проверяем
    // сам контракт отказов у fake и то, что движок идёт обычным путём при любом `Err`.
    let mut rig = warm_ready_rig();
    // Тёплая «пропала» у бэкенда (платформа закрыла её без события): обмен будет отвергнут.
    rig.lens.rig.fake.close_standby_silently(slot(0));
    rig.tap_while_warm(0.5);
    let calls = rig.calls();
    assert!(calls.contains(&"swap_standby_with_primary"), "{calls:?}");
    assert!(calls.contains(&"open_device"), "обычный путь после отказа: {calls:?}");
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    assert_eq!(rig.standby().counters.promotion_fallbacks, 1);
    assert_eq!(rig.standby().counters.promotions, 0);
    assert_eq!(
        StandbySwapFailure::StandbyNotStreaming.to_string(),
        "у тёплой камеры нет идущего потока"
    );
}

/// Основная стримит ширик, его сосед (основная камера) греется после тапа вниз без тёплой.
fn streaming_on_ultra_wide_with_wide_warming() -> StandbyRig {
    let mut rig = StandbyRig::streaming_on_wide(StandbyRigSetup::default());
    rig.intent(0.5, ZoomGesture::Discrete, StandbyDemand::KeepNeighbourWarm);
    rig.lens.frames(1);
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    assert_eq!(rig.slot_state(0), StandbySlotState::Warming);
    rig.lens.rig.fresh_events();
    rig.calls();
    rig
}

#[test]
fn wheel_at_exactly_one_x_shows_the_main_camera_only_when_it_is_warm_and_ready() {
    let mut rig = streaming_on_ultra_wide_with_wide_warming();

    // Тёплая основная ещё греется: смена — переоткрытие, гистерезис держит ширик на 1.0.
    rig.wheel_at(1.0);
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    assert!(!rig.calls().contains(&"swap_standby_with_primary"));

    // Готова: то же деление колеса 1.0 обменом ролей даёт основную камеру.
    rig.make_standby_ready(0);
    rig.calls();
    rig.wheel_at(1.0);
    let calls = rig.calls();
    assert!(calls.contains(&"swap_standby_with_primary"), "{calls:?}");
    assert!(!calls.contains(&"open_device"), "{calls:?}");
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(wide_id()));
    rig.lens.frames(1);
    assert_eq!(
        switched_method(&rig.lens.rig.fresh_events()),
        vec![(LensRole::UltraWide, LensRole::Wide, LensSwitchMethod::PromotedStandby)]
    );
}

#[test]
fn ready_warm_main_camera_does_not_take_over_below_its_own_ratio() {
    let mut rig = streaming_on_ultra_wide_with_wide_warming();
    rig.make_standby_ready(0);
    rig.calls();
    // Основная не умеет показать меньше 1×: на 0.99 остаётся ширик, обмена нет.
    rig.wheel_at(0.99);
    assert!(!rig.calls().contains(&"swap_standby_with_primary"));
    assert_eq!(rig.lens.rig.fake.open_camera(), Some(ultra_wide_id()));
}
