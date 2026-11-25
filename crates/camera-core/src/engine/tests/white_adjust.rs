//! Команда `AdjustWhite`: правка одной оси белой. Вторая ось — у владельца, а не у экрана (гонка с ответом
//! источника), дальше всё как у `SetWhite`: рука сильнее, прижим, «уже так», работа без потока.

use camera_contract::white::{WhiteAdjustment, WhiteOrigin, WhiteSourceId, WhiteTransition};

use super::white_support::*;
use super::*;
use crate::command::InvalidStateReason;
use crate::three_a_status::{AdjustmentReason, ControlParameter};
use crate::white::{
    RememberedOrigin, WhiteCancelCause, WhiteMemoryRecord, WhiteRequestOutcome, WhiteRequestState, WhiteTakeState,
};

fn temperature(kelvin: f64) -> WhiteAdjustment {
    WhiteAdjustment::Temperature(setting(kelvin, 0.0).temperature)
}

fn tint(duv: f64) -> WhiteAdjustment {
    WhiteAdjustment::Tint(setting(5_000.0, duv).tint)
}

fn adjust(adjustment: WhiteAdjustment) -> CameraCommand {
    CameraCommand::AdjustWhite {
        adjustment,
        transition: WhiteTransition::Immediate,
    }
}

#[test]
fn adjusting_one_axis_keeps_the_other_bit_for_bit_and_works_without_a_camera() {
    let (mut rig, outside) = white_rig();
    assert_eq!(rig.state_name(), "closed", "потока нет");
    // Значения, которые не округляются «красиво»: бит-в-бит видно только на них.
    rig.send(CameraCommand::SetWhite {
        setting: setting(4_321.987_654_321, -0.012_345_678_9),
        transition: WhiteTransition::Immediate,
    });
    let before = rig.held().setting;

    assert_eq!(rig.send(adjust(temperature(6_100.123_456))), CommandOutcome::Accepted);
    let after_temperature = rig.held();
    assert_eq!(after_temperature.setting.temperature.kelvin(), 6_100.123_456);
    assert_eq!(
        after_temperature.setting.tint.duv().to_bits(),
        before.tint.duv().to_bits(),
        "оттенок не тронут ни на бит"
    );
    assert_eq!(after_temperature.origin, WhiteOrigin::Manual);
    assert_eq!(
        outside.directive().white,
        after_temperature.setting,
        "рендер получил итог"
    );

    assert_eq!(rig.send(adjust(tint(0.031_415_926))), CommandOutcome::Accepted);
    let after_tint = rig.held().setting;
    assert_eq!(after_tint.tint.duv(), 0.031_415_926);
    assert_eq!(
        after_tint.temperature.kelvin().to_bits(),
        after_temperature.setting.temperature.kelvin().to_bits(),
        "температура не тронута ни на бит"
    );
    assert_eq!(
        outside.saved().last().copied(),
        Some(WhiteMemoryRecord {
            setting: after_tint,
            origin: RememberedOrigin::Manual
        }),
        "в память ушла белая целиком"
    );
}

#[test]
fn an_adjustment_right_after_the_source_answer_builds_on_the_source_axis_not_on_stale_screen_data() {
    // Гонка: экран ещё показывает прежнюю белую, а владелец уже принял ответ источника. Правка оттенка не должна
    // вернуть температуру к прежнему числу.
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.012)));
    assert_eq!(rig.held().origin, WhiteOrigin::Source(WhiteSourceId::PHONE));

    assert_eq!(rig.send(adjust(tint(-0.02))), CommandOutcome::Accepted);

    let held = rig.held();
    assert_eq!(held.setting.temperature.kelvin(), 3_700.0, "ось источника сохранена");
    assert_eq!(held.setting.tint.duv(), -0.02);
    assert_eq!(held.origin, WhiteOrigin::Manual, "после правки белая ручная");
}

#[test]
fn an_adjustment_during_a_wait_cancels_the_request_and_the_late_answer_is_foreign() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });
    let take = rig.last_command_id();
    rig.fresh_events();

    assert_eq!(rig.send(adjust(temperature(4_800.0))), CommandOutcome::Accepted);

    assert_eq!(outside.directive().request, None, "заявка снята");
    let results = finished_requests(&rig.fresh_events());
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].result,
        WhiteRequestOutcome::Cancelled {
            by: WhiteCancelCause::ManualWhite
        }
    );
    assert_eq!(
        rig.white_status()
            .last_take
            .map(|report| (report.command, report.state)),
        Some((
            take,
            WhiteTakeState::Finished(WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::ManualWhite
            })
        ))
    );
    rig.deliver_answer(&outside, ready(2, setting(3_000.0, 0.0)));
    assert_eq!(rig.white_status().stale_answers, 1);
    assert_eq!(
        rig.held().setting.temperature.kelvin(),
        4_800.0,
        "опоздавший ответ белую не сменил"
    );
}

#[test]
fn each_axis_is_clamped_to_the_config_range_and_the_other_axis_is_untouched() {
    let (mut rig, _outside) = white_rig();
    rig.send(CameraCommand::SetWhite {
        setting: setting(5_000.0, 0.01),
        transition: WhiteTransition::Immediate,
    });
    rig.fresh_events();

    // Температура выше предела (12000).
    assert_eq!(rig.send(adjust(temperature(20_000.0))), CommandOutcome::Accepted);
    assert_eq!(rig.held().setting, setting(12_000.0, 0.01));
    let limits = rig.white_status().limits;
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0].parameter, ControlParameter::WhiteBalanceTemperature);
    assert_eq!(limits[0].requested, 20_000.0);
    assert_eq!(limits[0].applied, 12_000.0);
    assert_eq!(limits[0].reason, AdjustmentReason::AboveWhiteLimit);

    // Температура ниже предела (2000).
    rig.send(adjust(temperature(500.0)));
    assert_eq!(rig.held().setting, setting(2_000.0, 0.01));
    assert_eq!(rig.white_status().limits[0].reason, AdjustmentReason::BelowWhiteLimit);

    // Оттенок: плюс и минус.
    rig.send(adjust(tint(0.2)));
    assert_eq!(rig.held().setting, setting(2_000.0, 0.05));
    let limits = rig.white_status().limits;
    assert_eq!(limits.len(), 1, "прижим температуры прошлой просьбы не тянется");
    assert_eq!(limits[0].parameter, ControlParameter::WhiteBalanceTint);
    assert_eq!(limits[0].reason, AdjustmentReason::AboveWhiteLimit);
    rig.send(adjust(tint(-0.2)));
    assert_eq!(rig.held().setting, setting(2_000.0, -0.05));
    assert_eq!(rig.white_status().limits[0].reason, AdjustmentReason::BelowWhiteLimit);

    // Прижим виден и событием.
    let adjusted_events = rig
        .fresh_events()
        .into_iter()
        .filter(|event| matches!(event, CameraEvent::ControlsAdjusted { .. }))
        .count();
    assert_eq!(adjusted_events, 4);

    // В пределах поправок нет.
    rig.send(adjust(tint(0.0)));
    assert!(rig.white_status().limits.is_empty());
}

#[test]
fn the_same_adjustment_twice_is_already_in_state_and_republishes_nothing() {
    let (mut rig, outside) = white_rig();
    assert_eq!(rig.send(adjust(temperature(4_000.0))), CommandOutcome::Accepted);
    let generation = outside.generation();
    let saved = outside.saved().len();

    assert_eq!(rig.send(adjust(temperature(4_000.0))), CommandOutcome::AlreadyInState);

    assert_eq!(outside.generation(), generation);
    assert_eq!(outside.saved().len(), saved);
}

#[test]
fn an_adjustment_that_equals_a_source_white_still_makes_it_manual() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.0)));

    assert_eq!(
        rig.send(adjust(temperature(3_700.0))),
        CommandOutcome::Accepted,
        "число то же, но происхождение сменилось: рука сильнее источника"
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Manual);
}

#[test]
fn an_adjustment_while_waiting_even_with_the_same_value_cancels_the_wait() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.send(adjust(temperature(4_000.0)));
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });

    assert_eq!(rig.send(adjust(temperature(4_000.0))), CommandOutcome::Accepted);
    assert!(matches!(rig.white_status().request, WhiteRequestState::Idle { .. }));
    assert_eq!(outside.directive().request, None);
}

#[test]
fn an_adjustment_in_fatal_is_rejected_and_the_white_is_unchanged() {
    let fake = FakeBackendControl::new();
    fake.set_catalog_failure(Some(crate::backend::BackendFailure {
        operation: "ACameraManager_getCameraIdList",
        code: -1,
        detail: "служба камеры недоступна".to_owned(),
    }));
    let (white, _outside) = white_wiring(crate::white::WhiteMemoryLoad::NothingRemembered);
    let config = CameraEngineConfig::default();
    let wiring = ThreeAWiring::vendor(&config.three_a).with_white(white);
    let mut rig = Rig::over_fake_with_wiring(config, AppPresence::Foreground, fake, wiring);
    rig.engine.load_catalog(rig.now);
    assert!(rig.engine.is_fatal());
    let before = rig.held();

    assert_eq!(
        rig.send(adjust(temperature(4_000.0))),
        CommandOutcome::InvalidState(InvalidStateReason::EngineFailed)
    );
    assert_eq!(rig.held(), before);
}

#[test]
fn going_to_background_flushes_memory_once_after_new_saves_and_not_without_them() {
    let (mut rig, outside) = white_rig_streaming();
    assert_eq!(outside.flushes(), 0);
    rig.send(CameraCommand::EnterBackground);
    assert_eq!(outside.flushes(), 0, "ничего не записывалось — сбрасывать нечего");
    rig.send(CameraCommand::EnterForeground);

    for step in 0..50_u32 {
        rig.send(adjust(temperature(3_000.0 + f64::from(step) * 10.0)));
    }
    assert_eq!(outside.flushes(), 0, "под пальцем память не торопят");
    rig.send(CameraCommand::EnterBackground);
    assert_eq!(outside.flushes(), 1, "ушли в фон: последнее значение ложится сразу");

    rig.send(CameraCommand::EnterForeground);
    rig.send(CameraCommand::EnterBackground);
    assert_eq!(outside.flushes(), 1, "новых записей не было — второго сброса нет");
}
