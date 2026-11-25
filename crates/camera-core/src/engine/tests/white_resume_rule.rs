//! Правило возобновления белого (WB36): через команды и события движка. Рендер заменён чтением указаний и
//! ящиком ответов; «белая не изменилась ни на бит» проверяется по указанию рендеру и по статусу.

use camera_contract::white::{
    HeldWhite, WhileAwaitingWhite, WhiteOrigin, WhiteRequestId, WhiteSourceId, WhiteTransition,
};

use super::white_support::*;
use super::*;
use crate::white::{
    RememberedOrigin, WhiteMemoryLoad, WhiteMemoryRecord, WhiteRequestOutcome, WhiteRequestState, WhiteResumeRule,
};

const SECOND: Duration = Duration::from_secs(1);

fn phone_held(kelvin: f64, duv: f64) -> HeldWhite {
    HeldWhite {
        setting: setting(kelvin, duv),
        origin: WhiteOrigin::Source(WhiteSourceId::PHONE),
    }
}

fn set_rule(rig: &mut Rig, rule: WhiteResumeRule) -> CommandOutcome {
    rig.send(CameraCommand::SetWhiteResumeRule { rule })
}

/// Стенд `keep_last` с запомненной белой телефона 3700 К: как после прошлого запуска.
fn keep_last_rig_remembering_phone_white() -> (Rig, WhiteOutside) {
    white_rig_with_rule(
        WhiteResumeRule::KeepLast,
        WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
            setting: setting(3_700.0, 0.01),
            origin: RememberedOrigin::Source,
        }),
        None,
    )
}

// ---- keep_last: возобновление не трогает белую ----

#[test]
fn keep_last_resume_after_a_pause_asks_nobody_hides_nothing_and_keeps_the_white_bit_for_bit() {
    let (mut rig, outside) = keep_last_rig_remembering_phone_white();
    let held = rig.held();
    let generation = outside.generation();
    rig.fresh_events();

    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    rig.start_streaming_on_wide();
    rig.frame_after(SECOND);
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    rig.frame_after(SECOND);

    assert_eq!(rig.held(), held, "белая та же, с тем же происхождением");
    assert_eq!(outside.directive().white, held.setting, "рендеру указана та же белая");
    assert_eq!(outside.directive().request, None, "заявки нет: кадры не скрыты");
    assert_eq!(
        outside.generation(),
        generation,
        "указание рендеру не публиковалось заново"
    );
    assert_eq!(
        rig.white_status().request,
        WhiteRequestState::Idle { last: None },
        "источник не спрошен ни разу"
    );
    assert_eq!(rig.engine.next_deadline(), None, "срока ожидания нет");
    assert!(
        finished_requests(&rig.fresh_events()).is_empty(),
        "ни одной заявки не закрыто"
    );
}

#[test]
fn keep_last_lost_stream_and_lens_switch_do_not_change_the_white_or_ask_the_source() {
    let (mut rig, outside) = keep_last_rig_remembering_phone_white();
    rig.send(CameraCommand::EnterForeground);
    rig.start_streaming_on_wide();
    rig.frame_after(SECOND);
    let held = rig.held();
    let generation = outside.generation();

    rig.send(CameraCommand::StopStreaming);
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(
        rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Accepted
    );
    rig.frame_after(SECOND);

    assert_eq!(rig.held(), held);
    assert_eq!(outside.generation(), generation, "указание не менялось");
    assert_eq!(outside.directive().request, None);
    assert_eq!(rig.white_status().request, WhiteRequestState::Idle { last: None });
}

#[test]
fn keep_last_keeps_a_manual_white_and_a_phone_white_alike() {
    let (mut rig, outside) = white_rig_with_rule(WhiteResumeRule::KeepLast, WhiteMemoryLoad::NothingRemembered, None);
    // Самый первый запуск: запасной свет, первое возобновление спрашивает (WB38); рука отменяет заявку.
    rig.send(CameraCommand::EnterForeground);
    rig.send(CameraCommand::SetWhite {
        setting: setting(4_100.0, -0.01),
        transition: WhiteTransition::Immediate,
    });
    let held = rig.held();
    assert_eq!(held.origin, WhiteOrigin::Manual);
    let generation = outside.generation();

    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);

    assert_eq!(rig.held(), held, "ручное держится");
    assert_eq!(outside.directive().request, None);
    assert_eq!(outside.generation(), generation);
}

// ---- keep_last: запасной свет — не выбор человека (WB38, WB39) ----

#[test]
fn keep_last_first_launch_asks_the_phone_once_then_holds_its_answer() {
    let (mut rig, outside) = white_rig_with_rule(WhiteResumeRule::KeepLast, WhiteMemoryLoad::NothingRemembered, None);
    rig.send(CameraCommand::EnterForeground);
    let request = outside
        .directive()
        .request
        .expect("первый запуск: белая запасная, телефон спрошен");
    assert_eq!(
        request.while_awaiting,
        WhileAwaitingWhite::HideFrames,
        "кадры скрыты до ответа"
    );
    rig.start_streaming_on_wide();
    rig.frame_after(SECOND);
    rig.deliver_answer(&outside, ready(request.id.0, setting(3_300.0, 0.004)));
    assert_eq!(rig.held(), phone_held(3_300.0, 0.004));

    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);

    assert_eq!(
        outside.directive().request,
        None,
        "ответ телефона больше не спрашивается"
    );
    assert_eq!(rig.held(), phone_held(3_300.0, 0.004));
}

#[test]
fn keep_last_asks_again_while_the_white_is_still_the_fallback_and_stops_after_an_answer() {
    let (mut rig, outside) = white_rig_with_rule(WhiteResumeRule::KeepLast, WhiteMemoryLoad::NothingRemembered, None);
    rig.send(CameraCommand::EnterForeground);
    rig.start_streaming_on_wide();
    rig.frame_after(SECOND);
    // Телефон не ответил: срок вышел, запасной свет остался.
    rig.advance(Duration::from_millis(3_100));
    let results = finished_requests(&rig.fresh_events());
    assert_eq!(results.last().map(|r| r.result), Some(WhiteRequestOutcome::TimedOut));
    assert_eq!(rig.held().origin, WhiteOrigin::Fallback);

    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    let second = outside
        .directive()
        .request
        .expect("запасной свет: спросить снова (WB39)");
    assert_eq!(second.id, WhiteRequestId(2));

    // Отказ источника («не знаю») тоже оставляет запасной свет и ведёт к новому вопросу.
    rig.frame_after(SECOND);
    rig.deliver_answer(
        &outside,
        answer(
            2,
            camera_contract::white::WhiteSourceOutcome::Unknown {
                reason: camera_contract::white::WhiteUnknownReason::SceneNotMeasurable,
            },
        ),
    );
    assert_eq!(rig.held().origin, WhiteOrigin::Fallback);
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    let third = outside.directive().request.expect("снова спрашиваем");
    assert_eq!(third.id, WhiteRequestId(3));

    // Ответ получен — дальше не спрашиваем.
    rig.frame_after(SECOND);
    rig.deliver_answer(&outside, ready(3, setting(5_000.0, 0.0)));
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    assert_eq!(outside.directive().request, None);
    assert_eq!(rig.held(), phone_held(5_000.0, 0.0));
}

// ---- keep_last: источник по просьбе человека ----

#[test]
fn keep_last_still_takes_the_phone_white_on_request() {
    let (mut rig, outside) = keep_last_rig_remembering_phone_white();
    rig.send(CameraCommand::EnterForeground);
    rig.start_streaming_on_wide();
    rig.frame_after(SECOND);

    assert_eq!(
        rig.send(CameraCommand::TakeWhite {
            source: WhiteSourceId::PHONE,
            transition: WhiteTransition::Immediate,
        }),
        CommandOutcome::Accepted
    );
    let request = outside.directive().request.expect("заявка команды");
    assert_eq!(request.while_awaiting, WhileAwaitingWhite::KeepShowing);
    rig.deliver_answer(&outside, ready(request.id.0, setting(6_200.0, 0.0)));

    assert_eq!(rig.held(), phone_held(6_200.0, 0.0), "по просьбе белая взята");
}

// ---- смена правила на ходу ----

#[test]
fn changing_the_rule_with_a_pending_request_leaves_the_request_and_the_white_alone() {
    // По умолчанию take_from_source: возобновление завело заявку.
    let (mut rig, outside) = white_rig();
    rig.send(CameraCommand::EnterForeground);
    rig.start_streaming_on_wide();
    let held = rig.held();
    let generation = outside.generation();
    let status_before = rig.white_status();

    assert_eq!(set_rule(&mut rig, WhiteResumeRule::KeepLast), CommandOutcome::Accepted);

    assert_eq!(outside.generation(), generation, "указание не публиковалось заново");
    assert_eq!(rig.held(), held);
    assert_eq!(
        rig.white_status().request,
        status_before.request,
        "заявка прежняя, ждёт ответа"
    );
    assert_eq!(outside.directive().request.map(|r| r.id), Some(WhiteRequestId(1)));
    // Ответ по-прежнему принимается: заявка не отменялась.
    rig.frame_after(SECOND);
    rig.deliver_answer(&outside, ready(1, setting(3_900.0, 0.0)));
    assert_eq!(rig.held(), phone_held(3_900.0, 0.0));
    // А вот со следующего возобновления правило действует.
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    assert_eq!(outside.directive().request, None);
}

#[test]
fn switching_back_to_take_from_source_takes_effect_on_the_next_resume() {
    let (mut rig, outside) = keep_last_rig_remembering_phone_white();
    assert_eq!(
        set_rule(&mut rig, WhiteResumeRule::TakeFromSource),
        CommandOutcome::Accepted
    );
    assert_eq!(rig.white_status().resume_rule, WhiteResumeRule::TakeFromSource);
    rig.send(CameraCommand::EnterForeground);
    assert_eq!(
        outside.directive().request.map(|r| r.while_awaiting),
        Some(WhileAwaitingWhite::HideFrames)
    );
}

#[test]
fn the_rule_can_be_chosen_in_the_background_with_no_stream() {
    let (mut rig, outside) = white_rig();
    let held = rig.held();
    let generation = outside.generation();
    rig.fresh_events();

    assert_eq!(set_rule(&mut rig, WhiteResumeRule::KeepLast), CommandOutcome::Accepted);

    assert_eq!(rig.white_status().resume_rule, WhiteResumeRule::KeepLast);
    assert_eq!(rig.held(), held);
    assert_eq!(outside.generation(), generation);
    let changed: Vec<WhiteResumeRule> = rig
        .fresh_events()
        .iter()
        .filter_map(|event| match event {
            CameraEvent::WhiteResumeRuleChanged { rule } => Some(*rule),
            _ => None,
        })
        .collect();
    assert_eq!(changed, vec![WhiteResumeRule::KeepLast], "ровно одно событие о выборе");
}

// ---- AlreadyInState и выбор, совпавший с умолчанием ----

#[test]
fn repeating_the_same_choice_is_already_in_state_and_writes_nothing_twice() {
    let (mut rig, outside) = white_rig();
    assert_eq!(set_rule(&mut rig, WhiteResumeRule::KeepLast), CommandOutcome::Accepted);
    rig.fresh_events();

    assert_eq!(
        set_rule(&mut rig, WhiteResumeRule::KeepLast),
        CommandOutcome::AlreadyInState
    );

    assert_eq!(
        outside.saved_rules(),
        vec![WhiteResumeRule::KeepLast],
        "в память — один раз"
    );
    assert!(
        !rig.fresh_events()
            .iter()
            .any(|event| matches!(event, CameraEvent::WhiteResumeRuleChanged { .. })),
        "повтор события не даёт"
    );
}

#[test]
fn a_choice_equal_to_the_config_default_is_still_a_choice_and_is_remembered() {
    let (mut rig, outside) = white_rig();
    assert_eq!(
        rig.white_status().resume_rule,
        WhiteResumeRule::TakeFromSource,
        "умолчание конфига"
    );

    assert_eq!(
        set_rule(&mut rig, WhiteResumeRule::TakeFromSource),
        CommandOutcome::Accepted,
        "выбор человека, пусть и равный умолчанию"
    );
    assert_eq!(outside.saved_rules(), vec![WhiteResumeRule::TakeFromSource]);
    assert_eq!(
        set_rule(&mut rig, WhiteResumeRule::TakeFromSource),
        CommandOutcome::AlreadyInState
    );
}

// ---- память ----

#[test]
fn the_choice_goes_to_memory_counts_as_a_save_and_is_flushed_on_background() {
    let (mut rig, outside) = white_rig();
    assert_eq!(rig.white_status().memory.saves_requested, 0);

    set_rule(&mut rig, WhiteResumeRule::KeepLast);

    assert_eq!(outside.saved_rules(), vec![WhiteResumeRule::KeepLast]);
    assert!(outside.saved().is_empty(), "белая этим не записывается");
    assert_eq!(rig.white_status().memory.saves_requested, 1);
    rig.send(CameraCommand::EnterForeground);
    rig.send(CameraCommand::EnterBackground);
    assert_eq!(
        outside.flushes(),
        1,
        "несброшенный выбор ложится на диск при уходе в фон"
    );
}

#[test]
fn a_remembered_choice_beats_the_config_and_is_not_written_back() {
    let (mut rig, outside) = white_rig_with_rule(
        WhiteResumeRule::TakeFromSource,
        WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
            setting: setting(3_700.0, 0.01),
            origin: RememberedOrigin::Source,
        }),
        Some(WhiteResumeRule::KeepLast),
    );
    assert_eq!(rig.white_status().resume_rule, WhiteResumeRule::KeepLast);
    assert!(outside.saved_rules().is_empty(), "восстановление память не пишет");

    rig.send(CameraCommand::EnterForeground);
    assert_eq!(
        outside.directive().request,
        None,
        "правило из памяти действует после перезапуска"
    );
    assert_eq!(rig.held(), phone_held(3_700.0, 0.01));
}

#[test]
fn a_remembered_take_from_source_beats_a_keep_last_config() {
    let (mut rig, outside) = white_rig_with_rule(
        WhiteResumeRule::KeepLast,
        WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
            setting: setting(3_700.0, 0.01),
            origin: RememberedOrigin::Source,
        }),
        Some(WhiteResumeRule::TakeFromSource),
    );
    assert_eq!(rig.white_status().resume_rule, WhiteResumeRule::TakeFromSource);
    rig.send(CameraCommand::EnterForeground);
    assert!(outside.directive().request.is_some());
}

#[test]
fn without_a_remembered_choice_the_config_rule_decides() {
    let (rig, _outside) = white_rig_with_rule(WhiteResumeRule::KeepLast, WhiteMemoryLoad::NothingRemembered, None);
    assert_eq!(rig.white_status().resume_rule, WhiteResumeRule::KeepLast);
}

#[test]
fn the_initial_status_before_the_first_engine_state_shows_the_effective_rule() {
    let (white, _outside) = white_wiring(WhiteMemoryLoad::NothingRemembered);
    let config = crate::white::WhiteConfig {
        resume_rule: WhiteResumeRule::KeepLast,
        ..crate::white::WhiteConfig::default()
    };
    assert_eq!(
        crate::engine::initial_white_status(&config, &white).resume_rule,
        WhiteResumeRule::KeepLast
    );
    let remembering = crate::white::WhiteWiring {
        remembered_resume_rule: Some(WhiteResumeRule::TakeFromSource),
        ..white
    };
    assert_eq!(
        crate::engine::initial_white_status(&config, &remembering).resume_rule,
        WhiteResumeRule::TakeFromSource,
        "выбор из памяти сильнее конфига и в начальном снимке"
    );
}
