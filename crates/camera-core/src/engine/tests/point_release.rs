//! Точка тапа отпускается движком (решение 241): по наблюдению рендера («сцена сменилась», «регион ушёл за кадр»)
//! и при возврате из фона. Блокировка AE/AF точку держит, вспышка и серия наблюдение откладывают.

use camera_contract::auto_exposure::CameraTag;
use camera_contract::fixtures::camera_id;
use camera_contract::scene_change::{PointObsolete, PointObsoleteCause};
use camera_contract::three_a::{AeAfLock, PointOfInterest, PointRequestId};

use super::flash_support::{FlashRig, SHOT, frame_time};
use super::three_a_support::*;
use super::*;
use crate::engine::three_a::point_release::{PointReleaseOutcome, PointReleaseTrigger};
use crate::point_hold::{PointHold, PointReleaseCause};
use crate::request_controls::FocusPlan;
use crate::three_a_status::FocusProgress;

/// Конфиг движка с выбранным правилом удержания точки.
fn config_with(hold: PointHold) -> CameraEngineConfig {
    let mut config = CameraEngineConfig::default();
    config.three_a.point_hold = hold;
    config
}

/// Стенд под нашим AE (он читает ящик наблюдений) на основной камере «2».
fn own_rig(hold: PointHold) -> FlashRig {
    FlashRig::on_camera(config_with(hold), "2", wide(), |_| {})
}

/// Тап в точку; возвращает номер тапа.
fn tap_at_point(flash: &mut FlashRig, point: camera_contract::three_a::SensorPoint) -> PointRequestId {
    assert_eq!(
        flash.rig.send(CameraCommand::FocusAndExposeAt { point }),
        CommandOutcome::Accepted
    );
    PointRequestId(flash.rig.last_command_id().0)
}

/// Долгий тап: точка с блокировкой.
fn long_tap_at_center(flash: &mut FlashRig) -> PointRequestId {
    assert_eq!(
        flash
            .rig
            .send(CameraCommand::LockFocusAndExposureAt { point: center() }),
        CommandOutcome::Accepted
    );
    PointRequestId(flash.rig.last_command_id().0)
}

/// Рендер сообщил: точка `request` устарела (кадр наблюдения на камере `camera`).
fn observe_obsolete(flash: &mut FlashRig, request: PointRequestId, camera: &str, cause: PointObsoleteCause) {
    let observation = PointObsolete {
        request,
        measured_on: CameraTag::of(&camera_id(camera)),
        frame: frame_time(flash.next_frame),
        cause,
    };
    flash.channel.publish_point_obsolete(observation);
    flash.deliver_pushes();
}

fn released_events(events: &[CameraEvent]) -> Vec<(PointRequestId, PointReleaseCause)> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::PointOfInterestReleased { request, cause } => Some((*request, *cause)),
            _ => None,
        })
        .collect()
}

fn three_a_state(flash: &FlashRig) -> (PointOfInterest, AeAfLock) {
    let status = flash.rig.engine.status().three_a;
    (status.point, status.lock)
}

fn is_point_of(state: (PointOfInterest, AeAfLock), request: PointRequestId) -> bool {
    matches!(state.0, PointOfInterest::Point { request: held, .. } if held == request)
}

#[test]
fn a_scene_change_releases_the_unlocked_point_once_and_the_render_gets_the_whole_frame_back() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let request = tap_at_point(&mut flash, upper_left());
    flash.rig.fresh_events();
    flash.rig.fake.take_calls();

    observe_obsolete(&mut flash, request, "2", PointObsoleteCause::SceneChanged);

    assert_eq!(three_a_state(&flash), (PointOfInterest::WholeFrame, AeAfLock::Unlocked));
    assert_eq!(
        released_events(&flash.rig.fresh_events()),
        vec![(request, PointReleaseCause::SceneChanged)]
    );
    let plans = repeating_plans(&flash.rig.fake.take_calls());
    assert_eq!(plans.len(), 1, "запрос перестроен ровно один раз");
    assert_eq!(
        plans[0].focus,
        FocusPlan::Continuous { region: None },
        "автофокус непрерывный"
    );
    assert_eq!(
        flash.directive().metering,
        PointOfInterest::WholeFrame,
        "рендер снова мерит весь кадр"
    );
    let status = flash.rig.engine.status().three_a;
    assert_eq!(status.focus, FocusProgress::Continuous { af_state: None });
    assert_eq!(
        (status.counters.points_released, status.counters.point_obsolete_ignored),
        (1, 0)
    );

    // То же наблюдение не разбирается второй раз: ни события, ни перестройки запроса.
    flash.rig.engine.refresh_three_a(flash.rig.now);
    flash.rig.pump();
    assert!(released_events(&flash.rig.fresh_events()).is_empty());
    assert!(repeating_plans(&flash.rig.fake.take_calls()).is_empty());
    assert_eq!(flash.rig.engine.status().three_a.counters.points_released, 1);
}

#[test]
fn a_region_leaving_the_frame_releases_the_point_with_its_own_cause() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let request = tap_at_point(&mut flash, upper_left());
    flash.rig.fresh_events();

    observe_obsolete(&mut flash, request, "2", PointObsoleteCause::RegionOutsideFrame);

    assert_eq!(
        released_events(&flash.rig.fresh_events()),
        vec![(request, PointReleaseCause::RegionOutsideFrame)]
    );
    assert_eq!(three_a_state(&flash).0, PointOfInterest::WholeFrame);
}

#[test]
fn a_late_observation_about_an_older_tap_does_not_release_the_new_point() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let old = tap_at_point(&mut flash, upper_left());
    let current = tap_at_point(&mut flash, center());
    flash.rig.fresh_events();
    flash.rig.fake.take_calls();

    observe_obsolete(&mut flash, old, "2", PointObsoleteCause::SceneChanged);

    assert!(is_point_of(three_a_state(&flash), current), "новая точка осталась");
    assert!(released_events(&flash.rig.fresh_events()).is_empty());
    assert!(
        repeating_plans(&flash.rig.fake.take_calls()).is_empty(),
        "запрос не трогали"
    );
    let counters = flash.rig.engine.status().three_a.counters;
    assert_eq!((counters.points_released, counters.point_obsolete_ignored), (0, 1));
}

#[test]
fn the_lock_keeps_the_point_and_the_lock_when_the_scene_changes() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let request = long_tap_at_center(&mut flash);
    flash.rig.fresh_events();
    flash.rig.fake.take_calls();

    observe_obsolete(&mut flash, request, "2", PointObsoleteCause::SceneChanged);

    let state = three_a_state(&flash);
    assert!(is_point_of(state, request));
    assert_eq!(state.1, AeAfLock::Locked, "блокировка держится");
    assert!(released_events(&flash.rig.fresh_events()).is_empty());
    assert!(repeating_plans(&flash.rig.fake.take_calls()).is_empty());
    assert_eq!(flash.rig.engine.status().three_a.counters.point_obsolete_ignored, 1);
}

#[test]
fn an_observation_measured_on_another_camera_is_ignored() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let request = tap_at_point(&mut flash, upper_left());
    flash.rig.fresh_events();

    // Поток идёт на камере «2»; наблюдение с ширика «3» к этой точке не относится.
    observe_obsolete(&mut flash, request, "3", PointObsoleteCause::SceneChanged);

    assert!(is_point_of(three_a_state(&flash), request));
    assert!(released_events(&flash.rig.fresh_events()).is_empty());
    assert_eq!(flash.rig.engine.status().three_a.counters.point_obsolete_ignored, 1);
}

#[test]
fn the_until_replaced_policy_keeps_the_old_behaviour_for_observations() {
    let mut flash = own_rig(PointHold::UntilReplaced);
    let request = tap_at_point(&mut flash, upper_left());
    flash.rig.fresh_events();

    observe_obsolete(&mut flash, request, "2", PointObsoleteCause::SceneChanged);

    assert!(
        is_point_of(three_a_state(&flash), request),
        "точка живёт до нового тапа"
    );
    assert!(released_events(&flash.rig.fresh_events()).is_empty());
    assert_eq!(flash.rig.engine.status().three_a.counters.points_released, 0);
}

#[test]
fn an_observation_without_a_point_changes_nothing_and_is_counted_as_ignored() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    flash.rig.fresh_events();
    flash.rig.fake.take_calls();

    observe_obsolete(&mut flash, PointRequestId(77), "2", PointObsoleteCause::SceneChanged);

    assert_eq!(three_a_state(&flash), (PointOfInterest::WholeFrame, AeAfLock::Unlocked));
    assert!(released_events(&flash.rig.fresh_events()).is_empty());
    assert!(repeating_plans(&flash.rig.fake.take_calls()).is_empty());
    assert_eq!(flash.rig.engine.status().three_a.counters.point_obsolete_ignored, 1);
}

#[test]
fn a_flash_in_progress_postpones_the_observation_and_it_is_applied_after_it_ends() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let request = tap_at_point(&mut flash, upper_left());
    assert_eq!(flash.capture(SHOT), CommandOutcome::Accepted);
    assert!(flash.rig.engine.status().three_a.flash.phase != crate::flash_status::FlashPhase::Idle);
    flash.rig.fresh_events();

    observe_obsolete(&mut flash, request, "2", PointObsoleteCause::SceneChanged);

    // Пока идёт снимок, наблюдение не читается: точка на месте, и в «разобранные» оно не попало.
    assert!(is_point_of(three_a_state(&flash), request));
    let counters = flash.rig.engine.status().three_a.counters;
    assert_eq!((counters.points_released, counters.point_obsolete_ignored), (0, 0));

    // Снимок отменён отказом движка вспышки? Достаточно закончить его по таймеру ожидания.
    flash.rig.advance(std::time::Duration::from_secs(60));
    assert_eq!(
        flash.rig.engine.status().three_a.flash.phase,
        crate::flash_status::FlashPhase::Idle,
        "вспышка закончилась"
    );
    flash.rig.engine.refresh_three_a(flash.rig.now);
    flash.rig.pump();

    assert_eq!(
        three_a_state(&flash).0,
        PointOfInterest::WholeFrame,
        "отложенное наблюдение применилось"
    );
    assert_eq!(
        released_events(&flash.rig.fresh_events()),
        vec![(request, PointReleaseCause::SceneChanged)]
    );
}

#[test]
fn a_bracket_in_progress_postpones_the_observation_and_it_is_applied_after_it_ends() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let request = tap_at_point(&mut flash, upper_left());
    let bracket = camera_contract::three_a::ExposureBracket::new(
        vec![camera_contract::three_a::BracketExposure {
            exposure_time: camera_contract::control::ExposureTime::new(std::time::Duration::from_millis(4))
                .expect("выдержка"),
            iso: camera_contract::control::Iso::new(100).expect("ISO"),
        }],
        32,
    )
    .expect("серия");
    assert_eq!(
        flash.rig.send(CameraCommand::CaptureBracket { bracket }),
        CommandOutcome::Accepted
    );
    let burst = camera_contract::three_a::BurstId(flash.rig.last_command_id().0);
    flash.rig.fresh_events();

    observe_obsolete(&mut flash, request, "2", PointObsoleteCause::SceneChanged);
    assert!(
        is_point_of(three_a_state(&flash), request),
        "серия идёт — точка на месте"
    );
    assert_eq!(flash.rig.engine.status().three_a.counters.point_obsolete_ignored, 0);

    flash.rig.fake.complete_bracket_frames(burst, &[0]);
    flash.rig.pump();
    flash.rig.engine.refresh_three_a(flash.rig.now);
    flash.rig.pump();

    assert_eq!(three_a_state(&flash).0, PointOfInterest::WholeFrame);
    assert_eq!(
        released_events(&flash.rig.fresh_events()),
        vec![(request, PointReleaseCause::SceneChanged)]
    );
}

#[test]
fn release_outcomes_are_told_apart_and_only_a_release_touches_the_intent() {
    let mut flash = own_rig(PointHold::UntilSceneChange);
    let observation = |request: PointRequestId, camera: &str| {
        PointReleaseTrigger::Observation(PointObsolete {
            request,
            measured_on: CameraTag::of(&camera_id(camera)),
            frame: frame_time(1),
            cause: PointObsoleteCause::SceneChanged,
        })
    };

    assert_eq!(
        flash.rig.engine.release_point(observation(PointRequestId(1), "2")),
        PointReleaseOutcome::NoPoint
    );
    let request = tap_at_point(&mut flash, upper_left());
    let before = three_a_state(&flash);
    assert_eq!(
        flash
            .rig
            .engine
            .release_point(observation(PointRequestId(request.0 + 100), "2")),
        PointReleaseOutcome::OtherRequest
    );
    assert_eq!(
        flash.rig.engine.release_point(observation(request, "3")),
        PointReleaseOutcome::ForeignCamera
    );
    assert_eq!(three_a_state(&flash), before, "отказы намерение не трогают");

    assert_eq!(
        flash.rig.engine.release_point(observation(request, "2")),
        PointReleaseOutcome::Released {
            request,
            cause: PointReleaseCause::SceneChanged
        }
    );
    assert_eq!(three_a_state(&flash), (PointOfInterest::WholeFrame, AeAfLock::Unlocked));

    let locked = long_tap_at_center(&mut flash);
    assert_eq!(
        flash.rig.engine.release_point(observation(locked, "2")),
        PointReleaseOutcome::HeldByLock
    );
    assert_eq!(flash.rig.engine.status().three_a.lock, AeAfLock::Locked);

    let mut replaced = own_rig(PointHold::UntilReplaced);
    let held = tap_at_point(&mut replaced, center());
    assert_eq!(
        replaced.rig.engine.release_point(observation(held, "2")),
        PointReleaseOutcome::HoldPolicy
    );
    assert!(is_point_of(three_a_state(&replaced), held));
}

#[test]
fn returning_from_the_background_releases_an_unlocked_point_and_does_not_restore_the_tap() {
    let mut rig = streaming_rig();
    assert_eq!(
        rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() }),
        CommandOutcome::Accepted
    );
    let request = PointRequestId(rig.last_command_id().0);
    assert_eq!(rig.send(CameraCommand::EnterBackground), CommandOutcome::Accepted);
    rig.fresh_events();
    rig.fake.take_calls();

    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);

    assert_eq!(rig.state_name(), "streaming");
    assert_eq!(rig.engine.status().three_a.point, PointOfInterest::WholeFrame);
    assert_eq!(
        released_events(&rig.fresh_events()),
        vec![(request, PointReleaseCause::ReturnedToApp)]
    );
    let calls = rig.fake.take_calls();
    assert!(triggers(&calls).is_empty(), "тап до фона заново не запускается");
    let plans = repeating_plans(&calls);
    assert_eq!(
        plans.last().expect("поток поднят").focus,
        FocusPlan::Continuous { region: None }
    );
    assert_eq!(rig.engine.status().three_a.counters.points_released, 1);
}

#[test]
fn returning_from_the_background_keeps_a_locked_point_and_its_lock() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    let request = PointRequestId(rig.last_command_id().0);
    rig.send(CameraCommand::EnterBackground);
    rig.fresh_events();

    rig.send(CameraCommand::EnterForeground);

    let status = rig.engine.status().three_a;
    assert_eq!(status.lock, AeAfLock::Locked);
    assert!(matches!(status.point, PointOfInterest::Point { request: held, .. } if held == request));
    assert!(released_events(&rig.fresh_events()).is_empty());
    assert_eq!(status.counters.points_released, 0);
}

#[test]
fn a_repeated_enter_foreground_without_a_background_does_not_release_the_point() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() });
    rig.fresh_events();

    rig.send(CameraCommand::EnterForeground);

    assert!(matches!(
        rig.engine.status().three_a.point,
        PointOfInterest::Point { .. }
    ));
    assert!(released_events(&rig.fresh_events()).is_empty());
}

#[test]
fn the_until_replaced_policy_keeps_the_point_across_the_background_too() {
    let mut rig = streaming_rig_with(config_with(PointHold::UntilReplaced));
    rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() });
    rig.send(CameraCommand::EnterBackground);
    rig.fresh_events();

    rig.send(CameraCommand::EnterForeground);

    assert!(matches!(
        rig.engine.status().three_a.point,
        PointOfInterest::Point { .. }
    ));
    assert!(released_events(&rig.fresh_events()).is_empty());
}

#[test]
fn a_lens_switch_still_resets_the_point_by_the_old_rule_and_is_not_counted_as_an_engine_release() {
    let mut rig = streaming_rig();
    rig.send(CameraCommand::FocusAndExposeAt { point: upper_left() });
    rig.fresh_events();

    rig.send(CameraCommand::SwitchTo {
        role: camera_contract::ids::LensRole::UltraWide,
    });

    let status = rig.engine.status().three_a;
    assert_eq!(
        (status.point, status.lock),
        (PointOfInterest::WholeFrame, AeAfLock::Unlocked)
    );
    assert!(
        released_events(&rig.fresh_events()).is_empty(),
        "это сброс по смене камеры, а не снятие по наблюдению"
    );
    assert_eq!(status.counters.points_released, 0);
}
