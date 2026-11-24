//! Ручная белая: пределы, повторы, память, переживание перезапуска и возврата из фона.

use camera_contract::white::{WhiteOrigin, WhiteRequestId, WhiteSourceId, WhiteTransition};

use super::white_support::*;
use super::*;
use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter};
use crate::white::{
    RememberedOrigin, WhiteCancelCause, WhiteMemoryLoad, WhiteMemoryRecord, WhiteMemoryStartup, WhiteRequestOutcome,
    WhiteRequestState,
};

fn set_white(kelvin: f64, duv: f64) -> CameraCommand {
    CameraCommand::SetWhite {
        setting: setting(kelvin, duv),
        transition: WhiteTransition::Immediate,
    }
}

fn adjusted_events(events: &[CameraEvent]) -> Vec<Vec<ControlAdjustment>> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::ControlsAdjusted { adjustments } => Some(adjustments.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn manual_white_is_published_remembered_and_works_without_a_camera() {
    let (mut rig, outside) = white_rig();
    assert_eq!(rig.state_name(), "closed");
    rig.fresh_events();

    assert_eq!(
        rig.send(CameraCommand::SetWhite {
            setting: setting(4_300.0, -0.006),
            transition: WhiteTransition::Over(Duration::from_millis(500)),
        }),
        CommandOutcome::Accepted
    );

    let directive = outside.directive();
    assert_eq!(directive.white, setting(4_300.0, -0.006));
    assert_eq!(
        directive.transition,
        WhiteTransition::Over(Duration::from_millis(500)),
        "переход команды дошёл до рендера"
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Manual);
    assert!(rig.white_status().limits.is_empty(), "в пределах прижима нет");
    assert_eq!(
        outside.saved(),
        vec![WhiteMemoryRecord {
            setting: setting(4_300.0, -0.006),
            origin: RememberedOrigin::Manual
        }]
    );
    let events = rig.fresh_events();
    assert_eq!(white_changes(&events), vec![rig.held()]);
    assert!(
        adjusted_events(&events).is_empty(),
        "без прижима нет и события о поправке"
    );
}

#[test]
fn the_same_manual_white_again_is_already_in_state_and_does_not_republish_or_resave() {
    let (mut rig, outside) = white_rig();
    rig.send(set_white(4_300.0, 0.0));
    let generation = outside.generation();

    assert_eq!(rig.send(set_white(4_300.0, 0.0)), CommandOutcome::AlreadyInState);
    assert_eq!(
        outside.generation(),
        generation,
        "указание не публикуется без изменений"
    );
    assert_eq!(outside.saved().len(), 1, "и в память второй раз не пишется");
    assert_eq!(rig.white_status().memory.saves_requested, 1);
}

#[test]
fn out_of_range_manual_white_is_pressed_to_the_limits_and_the_caller_sees_it() {
    let (mut rig, outside) = white_rig();
    rig.fresh_events();

    assert_eq!(rig.send(set_white(1_500.0, 0.2)), CommandOutcome::Accepted);

    let expected = vec![
        ControlAdjustment {
            parameter: ControlParameter::WhiteBalanceTemperature,
            requested: 1_500.0,
            applied: 2_000.0,
            reason: AdjustmentReason::BelowWhiteLimit,
        },
        ControlAdjustment {
            parameter: ControlParameter::WhiteBalanceTint,
            requested: 0.2,
            applied: 0.05,
            reason: AdjustmentReason::AboveWhiteLimit,
        },
    ];
    assert_eq!(rig.white_status().limits, expected, "прижим виден в статусе");
    assert_eq!(adjusted_events(&rig.fresh_events()), vec![expected], "и событием");
    assert_eq!(
        outside.directive().white,
        setting(2_000.0, 0.05),
        "рендеру уходит уже прижатое"
    );
    assert_eq!(
        outside.saved()[0].setting,
        setting(2_000.0, 0.05),
        "в память — прижатое"
    );

    // Вверх.
    rig.send(set_white(20_000.0, -0.3));
    assert_eq!(outside.directive().white, setting(12_000.0, -0.05));
    let limits = rig.white_status().limits;
    assert_eq!(limits.len(), 2);
    assert!(
        limits
            .iter()
            .all(|adjustment| adjustment.reason == AdjustmentReason::AboveWhiteLimit
                || adjustment.parameter == ControlParameter::WhiteBalanceTint)
    );
    assert_eq!(limits[0].applied, 12_000.0);
    assert_eq!(
        limits[1].reason,
        AdjustmentReason::BelowWhiteLimit,
        "оттенок поднят до -0,05"
    );

    // Повтор той же просьбы, дающей то же прижатое значение, — «уже так».
    assert_eq!(rig.send(set_white(30_000.0, -0.9)), CommandOutcome::AlreadyInState);
}

#[test]
fn a_manual_white_inside_the_limits_clears_the_previous_limit_report() {
    let (mut rig, _outside) = white_rig();
    rig.send(set_white(1_000.0, 0.0));
    assert_eq!(rig.white_status().limits.len(), 1);
    rig.send(set_white(5_000.0, 0.0));
    assert!(rig.white_status().limits.is_empty());

    // И когда белая не меняется: прижали к краю, а потом попросили ровно край.
    rig.send(set_white(1_000.0, 0.0));
    assert_eq!(rig.white_status().limits.len(), 1);
    assert_eq!(rig.send(set_white(2_000.0, 0.0)), CommandOutcome::AlreadyInState);
    assert!(rig.white_status().limits.is_empty(), "последняя просьба не прижималась");
}

#[test]
fn manual_white_survives_close_controls_lens_switch_and_a_return_from_background() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.send(set_white(4_300.0, 0.0));
    let held = rig.held();
    let generation = outside.generation();

    rig.send(CameraCommand::ApplyControls {
        controls: camera_contract::control::ManualControl::AUTOMATIC,
    });
    rig.send(CameraCommand::SwitchTo {
        role: LensRole::UltraWide,
    });
    rig.send(CameraCommand::Close);
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);

    assert_eq!(rig.held(), held, "рука держится всегда (WB22)");
    assert_eq!(
        outside.directive().request,
        None,
        "у ручной белой при возврате заявки нет"
    );
    assert_eq!(outside.generation(), generation, "указание не менялось ни разу");
    // Единственная заявка в журнале — открытие стенда, отменённое рукой; новых после возврата нет.
    assert!(matches!(
        rig.white_status().request,
        WhiteRequestState::Idle {
            last: Some(last)
        } if last.request == WhiteRequestId(1)
            && last.result == WhiteRequestOutcome::Cancelled { by: WhiteCancelCause::ManualWhite }
    ));
}

#[test]
fn a_manual_white_is_restored_after_a_process_restart_and_the_source_is_not_asked() {
    let (mut rig, outside) = white_rig_with(
        CameraEngineConfig::default(),
        AppPresence::Background,
        WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
            setting: setting(4_300.0, 0.01),
            origin: RememberedOrigin::Manual,
        }),
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Manual);
    assert_eq!(outside.directive().white, setting(4_300.0, 0.01));
    assert_eq!(rig.white_status().memory.startup, WhiteMemoryStartup::Restored);

    rig.send(CameraCommand::EnterForeground);
    assert_eq!(
        outside.directive().request,
        None,
        "ручная белая источником не перебивается"
    );
    assert_eq!(rig.white_status().request, WhiteRequestState::Idle { last: None });
}

#[test]
fn a_remembered_value_outside_the_limits_is_pressed_and_announced_at_start() {
    let (mut rig, outside) = white_rig_with(
        CameraEngineConfig::default(),
        AppPresence::Background,
        WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
            setting: setting(15_000.0, 0.0),
            origin: RememberedOrigin::Manual,
        }),
    );
    assert_eq!(outside.directive().white, setting(12_000.0, 0.0));
    let limits = rig.white_status().limits;
    assert_eq!(limits.len(), 1);
    assert_eq!((limits[0].requested, limits[0].applied), (15_000.0, 12_000.0));
    assert_eq!(
        adjusted_events(&rig.fresh_events()).len(),
        1,
        "прижим виден событием уже со старта"
    );
}

#[test]
fn an_unreadable_memory_gives_the_fallback_white_and_the_reason_is_visible() {
    let (rig, outside) = white_rig_with(
        CameraEngineConfig::default(),
        AppPresence::Background,
        WhiteMemoryLoad::Unreadable {
            reason: "файл обрезан".to_owned(),
        },
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Fallback);
    assert_eq!(outside.directive().white, setting(5_600.0, 0.0));
    assert_eq!(
        rig.white_status().memory.startup,
        WhiteMemoryStartup::Unreadable {
            reason: "файл обрезан".to_owned()
        }
    );
    assert!(outside.saved().is_empty(), "битое не перезаписывается само");
}

#[test]
fn the_fallback_light_comes_from_the_config_not_from_code() {
    let mut config = CameraEngineConfig::default();
    config.three_a.white.fallback_kelvin = 4_000.0;
    config.three_a.white.fallback_duv = -0.01;
    let (rig, outside) = white_rig_with(config, AppPresence::Background, WhiteMemoryLoad::NothingRemembered);
    assert_eq!(outside.directive().white, setting(4_000.0, -0.01));
    assert_eq!(rig.held().origin, WhiteOrigin::Fallback);
}

#[test]
fn a_source_white_outside_the_limits_is_pressed_and_visible_to_the_caller() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.fresh_events();
    rig.deliver_answer(&outside, ready(1, setting(1_500.0, 0.0)));

    assert_eq!(rig.held().setting, setting(2_000.0, 0.0));
    assert_eq!(rig.held().origin, WhiteOrigin::Source(WhiteSourceId::PHONE));
    let limits = rig.white_status().limits;
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0].reason, AdjustmentReason::BelowWhiteLimit);
    assert_eq!(adjusted_events(&rig.fresh_events()), vec![limits]);
    assert_eq!(outside.saved()[0].setting, setting(2_000.0, 0.0));
}

#[test]
fn unrelated_traffic_never_republishes_the_white_directive() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.0)));
    let generation = outside.generation();

    for _ in 0..20 {
        rig.frame_after(Duration::from_millis(16));
    }
    rig.send(CameraCommand::SetExposureBias {
        bias: camera_contract::control::ExposureBias::from_ev(0.5).expect("EV"),
    });
    rig.send(CameraCommand::LockFocusAndExposureAt {
        point: camera_contract::three_a::SensorPoint::CENTER,
    });
    rig.advance(Duration::from_secs(10));
    assert_eq!(
        outside.generation(),
        generation,
        "белая не менялась ни на бит, указание то же"
    );
    assert_eq!(rig.held().setting, setting(3_700.0, 0.0));
}
