//! Белая при открытии: заявка при каждом возврате из фона (WB22), срок от первого кадра, жизнь заявки
//! при смене потока. Всё через команды и события движка; рендер заменён чтением указаний и ящиком ответов.

use camera_contract::white::{
    HeldWhite, WhileAwaitingWhite, WhiteOrigin, WhiteRequestId, WhiteSourceId, WhiteTransition,
};

use super::white_support::*;
use super::*;
use crate::white::{WhiteCancelCause, WhiteMemoryLoad, WhiteRequestOutcome, WhiteRequestState, WhiteWiring};

const SECOND: Duration = Duration::from_secs(1);

fn fallback_held() -> HeldWhite {
    HeldWhite {
        setting: setting(5_600.0, 0.0),
        origin: WhiteOrigin::Fallback,
    }
}

#[test]
fn engine_publishes_the_fallback_white_at_creation_without_asking_anyone() {
    let (rig, outside) = white_rig();
    let (generation, directive) = outside.latest();
    assert_eq!(generation, 1, "указание опубликовано сразу");
    assert_eq!(
        directive.white,
        setting(5_600.0, 0.0),
        "запасной свет из конфига: 5600 К, Duv 0"
    );
    assert_eq!(
        directive.request, None,
        "спрашивать нечего, пока приложение не возобновилось"
    );
    assert_eq!(rig.held(), fallback_held());
    assert_eq!(rig.white_status().request, WhiteRequestState::Idle { last: None });
    assert!(outside.saved().is_empty(), "запасной свет в память не пишется");
}

#[test]
fn returning_to_foreground_asks_the_phone_and_hides_frames_until_the_answer() {
    let (mut rig, outside) = white_rig();
    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);

    let request = outside.directive().request.expect("заявка выдана");
    assert_eq!(request.id, WhiteRequestId(1));
    assert_eq!(request.source, WhiteSourceId::PHONE);
    assert_eq!(request.while_awaiting, WhileAwaitingWhite::HideFrames);
    assert_eq!(
        rig.white_status().request,
        WhiteRequestState::Waiting {
            request: WhiteRequestId(1),
            source: WhiteSourceId::PHONE
        }
    );
    assert_eq!(rig.held(), fallback_held(), "до ответа белая прежняя");
}

#[test]
fn a_repeated_enter_foreground_without_leaving_does_not_ask_again() {
    let (mut rig, outside) = white_rig();
    rig.send(CameraCommand::EnterForeground);
    let generation = outside.generation();

    assert_eq!(rig.send(CameraCommand::EnterForeground), CommandOutcome::AlreadyInState);
    assert_eq!(outside.generation(), generation, "указание не менялось");
    assert_eq!(
        outside.directive().request.map(|request| request.id),
        Some(WhiteRequestId(1))
    );
}

#[test]
fn every_return_from_background_asks_again_with_a_new_number() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(SECOND);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.01)));
    assert_eq!(outside.directive().request, None);
    rig.fresh_events();

    // Ушёл в фон на секунду и вернулся: WB22 — порога времени нет, берём заново.
    rig.send(CameraCommand::EnterBackground);
    assert_eq!(outside.directive().request, None, "в фоне спрашивать нечего");
    rig.send(CameraCommand::EnterForeground);
    let request = outside.directive().request.expect("новая заявка");
    assert_eq!(request.id, WhiteRequestId(2), "номера не переиспользуются");
    assert_eq!(request.while_awaiting, WhileAwaitingWhite::HideFrames);
    assert_eq!(
        rig.held().setting,
        setting(3_700.0, 0.01),
        "до нового ответа держится белая прошлого"
    );
}

#[test]
fn going_to_background_cancels_the_wait_and_keeps_the_white() {
    let (mut rig, outside) = white_rig();
    rig.send(CameraCommand::EnterForeground);
    rig.fresh_events();

    rig.send(CameraCommand::EnterBackground);
    let events = rig.fresh_events();
    assert_eq!(
        finished_requests(&events)
            .iter()
            .map(|result| result.result)
            .collect::<Vec<_>>(),
        vec![WhiteRequestOutcome::Cancelled {
            by: WhiteCancelCause::Background
        }]
    );
    assert_eq!(outside.directive().request, None);
    assert_eq!(rig.held(), fallback_held());

    // Ответ, пришедший после ухода в фон, чужой.
    rig.deliver_answer(&outside, ready(1, setting(3_000.0, 0.0)));
    assert_eq!(
        rig.held(),
        fallback_held(),
        "белая не меняется ответом отменённой заявки"
    );
    assert_eq!(rig.white_status().stale_answers, 1);
}

#[test]
fn the_wait_limit_counts_from_the_first_frame_not_from_the_request() {
    let (mut rig, outside) = white_rig();
    rig.send(CameraCommand::EnterForeground);
    rig.send(CameraCommand::Open { target: wide() });
    rig.send(CameraCommand::Configure { mode: mode_raw10_60() });
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(rig.engine.next_deadline(), None, "кадров ещё не было — срока нет");

    // Открытие камеры и диалог разрешения могут длиться долго: срок не сгорает.
    rig.advance(Duration::from_secs(20));
    assert_eq!(
        rig.white_status().request,
        WhiteRequestState::Waiting {
            request: WhiteRequestId(1),
            source: WhiteSourceId::PHONE,
        }
    );

    rig.frame_after(Duration::ZERO);
    let first_frame = rig.now;
    assert_eq!(rig.engine.next_deadline(), Some(first_frame + Duration::from_secs(3)));
    // Следующие кадры срок не двигают.
    rig.frame_after(SECOND);
    assert_eq!(rig.engine.next_deadline(), Some(first_frame + Duration::from_secs(3)));

    // Почти на сроке — ещё ждём; на сроке — «срок вышел», белая прежняя.
    rig.advance(Duration::from_millis(1_990));
    assert!(matches!(rig.white_status().request, WhiteRequestState::Waiting { .. }));
    rig.fresh_events();
    rig.advance(Duration::from_millis(20));
    let events = rig.fresh_events();
    let results = finished_requests(&events);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, WhiteRequestOutcome::TimedOut);
    assert_eq!(results[0].request, WhiteRequestId(1));
    assert_eq!(rig.held(), fallback_held(), "белая прежняя");
    assert_eq!(outside.directive().request, None, "заявка снята и с указания");
    assert_eq!(rig.engine.next_deadline(), None);
}

#[test]
fn an_answer_just_before_the_limit_is_accepted_once_and_just_after_is_dropped() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);

    rig.advance(Duration::from_millis(2_900));
    rig.deliver_answer(&outside, ready(1, setting(4_200.0, 0.0)));
    let directive = outside.directive();
    assert_eq!(directive.white, setting(4_200.0, 0.0), "позднее, но в срок — принято");
    assert_eq!(
        directive.transition,
        WhiteTransition::Over(Duration::from_millis(300)),
        "плавно за 0,3 с"
    );
    assert_eq!(directive.request, None);

    // Тот же ответ второй раз — чужой: принимается один раз.
    rig.deliver_answer(&outside, ready(1, setting(2_500.0, 0.0)));
    assert_eq!(rig.held().setting, setting(4_200.0, 0.0));
    assert_eq!(rig.white_status().stale_answers, 1);

    // Новый возврат из фона и опоздание за срок.
    rig.send(CameraCommand::EnterBackground);
    rig.send(CameraCommand::EnterForeground);
    rig.frame_after(Duration::ZERO);
    rig.advance(Duration::from_millis(3_100));
    rig.deliver_answer(&outside, ready(2, setting(9_000.0, 0.0)));
    assert_eq!(rig.held().setting, setting(4_200.0, 0.0), "после срока ответ отброшен");
    assert_eq!(rig.white_status().stale_answers, 2);
}

#[test]
fn a_stream_restart_reissues_the_request_with_a_new_number_and_the_old_stream_answer_is_foreign() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    assert!(rig.engine.next_deadline().is_some(), "срок идёт");

    rig.send(CameraCommand::StopStreaming);
    let request = outside.directive().request.expect("заявка не потеряна");
    assert_eq!(request.id, WhiteRequestId(2), "заявка переиздана с новым номером");
    assert_eq!(
        request.while_awaiting,
        WhileAwaitingWhite::HideFrames,
        "вид ожидания прежний"
    );
    assert_eq!(
        rig.engine.next_deadline(),
        None,
        "срок снят до первого кадра нового потока"
    );

    // Ответ прежнего потока опоздал: чужой.
    rig.deliver_answer(&outside, ready(1, setting(3_000.0, 0.0)));
    assert_eq!(rig.held(), fallback_held());
    assert_eq!(rig.white_status().stale_answers, 1);

    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    rig.frame_after(Duration::ZERO);
    assert_eq!(
        rig.engine.next_deadline(),
        Some(rig.now + Duration::from_secs(3)),
        "срок заново от первого кадра"
    );
    rig.deliver_answer(&outside, ready(2, setting(3_000.0, 0.0)));
    assert_eq!(rig.held().setting, setting(3_000.0, 0.0));
    assert_eq!(rig.held().origin, WhiteOrigin::Source(WhiteSourceId::PHONE));
}

#[test]
fn a_stream_that_ended_before_any_frame_does_not_burn_request_numbers() {
    let (mut rig, outside) = white_rig_streaming();
    // Кадров не было: потеря потока не меняет заявку (ответ потока без кадров невозможен).
    rig.send(CameraCommand::StopStreaming);
    rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(
        outside.directive().request.map(|request| request.id),
        Some(WhiteRequestId(1))
    );
}

#[test]
fn closing_and_controls_and_lens_switch_do_not_touch_the_white() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    rig.deliver_answer(&outside, ready(1, setting(3_700.0, 0.0)));
    let held = rig.held();
    let generation = outside.generation();
    let status = rig.white_status();

    rig.send(CameraCommand::ApplyControls {
        controls: camera_contract::control::ManualControl::AUTOMATIC,
    });
    assert_eq!(
        rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Accepted
    );
    rig.frame_after(SECOND);
    assert_eq!(rig.held(), held, "смена линзы белую держит (WB25)");
    assert_eq!(
        rig.white_status().request,
        status.request,
        "источник заново не спрашивается"
    );
    rig.send(CameraCommand::Close);
    assert_eq!(rig.held(), held, "закрытие белую не трогает");
    assert_eq!(outside.generation(), generation, "указание не публиковалось заново");
    assert_eq!(rig.white_status().limits, status.limits);
}

#[test]
fn remembered_phone_white_is_restored_and_asked_again_after_a_process_restart() {
    let remembered = crate::white::WhiteMemoryRecord {
        setting: setting(3_300.0, 0.004),
        origin: crate::white::RememberedOrigin::Source,
    };
    let (mut rig, outside) = white_rig_with(
        CameraEngineConfig::default(),
        AppPresence::Background,
        WhiteMemoryLoad::Remembered(remembered),
    );
    assert_eq!(
        rig.held(),
        HeldWhite {
            setting: setting(3_300.0, 0.004),
            origin: WhiteOrigin::Source(WhiteSourceId::PHONE)
        }
    );
    assert_eq!(
        outside.directive().white,
        setting(3_300.0, 0.004),
        "рендер видит запомненное с первого указания"
    );
    assert_eq!(outside.directive().request, None);
    assert!(outside.saved().is_empty(), "восстановление не пишет память заново");

    rig.send(CameraCommand::EnterForeground);
    assert_eq!(
        outside.directive().request.map(|request| request.id),
        Some(WhiteRequestId(1))
    );
}

#[test]
fn a_second_engine_over_the_same_answer_box_never_reuses_request_numbers() {
    let (mut first, outside) = white_rig_streaming();
    first.frame_after(Duration::ZERO);
    first.deliver_answer(&outside, ready(1, setting(3_000.0, 0.0)));

    // «Перезапуск»: новый движок над тем же ящиком ответов; в нём лежит ответ на заявку 1 прежнего движка.
    let wiring = WhiteWiring {
        answers: outside.answers.clone(),
        ..WhiteWiring::default()
    };
    let wiring = ThreeAWiring::vendor(&CameraEngineConfig::default().three_a).with_white(wiring);
    let mut second = Rig::over_fake_with_wiring(
        CameraEngineConfig::default(),
        AppPresence::Background,
        FakeBackendControl::new(),
        wiring,
    );
    second.engine.load_catalog(second.now);
    second.pump();
    second.send(CameraCommand::EnterForeground);
    second.engine.refresh_three_a(second.now);
    second.pump();

    assert_ne!(
        second.white_status().request,
        WhiteRequestState::Idle { last: None },
        "заявка нового движка ждёт"
    );
    assert_eq!(
        second.held().origin,
        WhiteOrigin::Fallback,
        "чужой старый ответ не принят"
    );
    assert_eq!(second.white_status().stale_answers, 1);
}
