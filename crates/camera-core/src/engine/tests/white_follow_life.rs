//! Слежение за белым источника: блокировка AE/AF, смена линзы, поток, фон, память, скорость и вспышка (WB46, WB50–WB56).

use camera_contract::white::{WhileAwaitingWhite, WhiteSourceOutcome, WhiteTransition};

use super::flash_support::{FlashRig, SHOT, ms};
use super::lens_support::LensRig;
use super::three_a_support::center;
use super::white_follow_support::*;
use super::white_support::*;
use super::*;
use crate::flash_status::FlashPhase;
use crate::white::{
    RememberedOrigin, WhiteFollowRecord, WhiteFollowSpeed, WhiteFollowState, WhiteKeeping, WhiteMemoryLoad,
    WhiteMemoryRecord, WhiteRequestState, WhiteResumeRule,
};

// ---- Блокировка AE/AF (WB46, WB54) ----

#[test]
fn the_lock_freezes_the_white_and_releasing_it_asks_from_the_very_next_frame() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.lock();
    assert_eq!(follow.state(), HELD_BY_LOCK);
    let (generation, asked) = (follow.outside.generation(), follow.asked());
    follow.frames(120);
    assert_eq!(
        follow.outside.generation(),
        generation,
        "под блокировкой ни одного указания"
    );
    assert_eq!(follow.asked(), asked);
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));

    follow.unlock();
    assert_eq!(follow.state(), FOLLOWING);
    follow.frames(1);
    follow.answer(setting(5_000.0, 0.0));
    assert_eq!(follow.rig.held(), phone(setting(5_000.0, 0.0)));
}

#[test]
fn an_answer_already_on_its_way_when_the_lock_lands_does_not_move_the_white() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let asked = follow.request().expect("вопрос висит").id.0;
    follow.lock();
    assert_eq!(follow.request(), None, "вопрос снят сразу, рендер его больше не видит");
    follow
        .rig
        .deliver_answer(&follow.outside, ready(asked, setting(9_000.0, 0.0)));
    assert_eq!(follow.rig.held(), phone(setting(4_000.0, 0.0)));
}

#[test]
fn a_human_action_under_the_lock_still_works_and_turns_auto_off() {
    let mut follow = FollowRig::following();
    follow.lock();
    follow.rig.send(CameraCommand::SetWhite {
        setting: setting(3_200.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    assert_eq!(follow.rig.held().setting, setting(3_200.0, 0.0));
    assert_eq!(follow.state(), WhiteFollowState::Off);
    follow.rig.send(CameraCommand::TakeWhite {
        source: PHONE,
        transition: WhiteTransition::Immediate,
    });
    follow.answer(setting(5_000.0, 0.0));
    assert_eq!(follow.rig.held(), phone(setting(5_000.0, 0.0)));
}

#[test]
fn a_lens_switch_drops_the_lock_and_the_white_follows_the_new_camera() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.lock();
    follow.rig.send(CameraCommand::SwitchTo {
        role: LensRole::UltraWide,
    });
    assert_eq!(follow.state(), FOLLOWING, "смена камеры блокировку снимает");
    assert_eq!(
        follow.outside.directive().white,
        setting(3_000.0, 0.0),
        "пока новая камера не ответила, белая держится (WB52)"
    );
    follow.frames(1);
    let request = follow.request().expect("новый поток спросил с первого кадра");
    assert_eq!(
        request.while_awaiting,
        WhileAwaitingWhite::KeepShowing,
        "кадры новой линзы не скрыты"
    );
    follow.answer(setting(3_400.0, 0.0));
    let directive = follow.outside.directive();
    assert_eq!(directive.white, setting(3_400.0, 0.0));
    assert_eq!(directive.transition, WhiteTransition::Over(Duration::from_millis(300)));
}

#[test]
fn releasing_the_lock_or_turning_auto_on_again_does_not_wait_for_the_rest_of_the_interval() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    follow.answer(setting(4_000.0, 0.0));
    follow.lock();
    follow.unlock();
    follow.frames(1);
    assert!(follow.request().is_some(), "сразу после снятия блокировки");

    follow.rig.send(CameraCommand::SetWhite {
        setting: setting(5_600.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    follow.follow();
    follow.frames(1);
    assert!(follow.request().is_some(), "сразу после повторного включения");
}

#[test]
fn a_lens_switch_by_the_screen_intent_releases_the_lock_for_the_white_too() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.send(CameraCommand::FollowWhite { source: PHONE });
    lens.rig.send(CameraCommand::LockFocusAndExposureAt { point: center() });
    let follow = |lens: &LensRig| lens.rig.engine.status().three_a.white.follow;
    assert_eq!(follow(&lens).state, HELD_BY_LOCK);
    lens.frames(10);
    assert_eq!(follow(&lens).asked, 0);

    lens.intent(camera_contract::ids::LensFacing::Back, 0.6);
    lens.frames(1);
    assert_eq!(follow(&lens).state, FOLLOWING);
    assert_eq!(follow(&lens).asked, 1, "новая камера спрошена с первого кадра");
}

// ---- Поток, фон, память ----

#[test]
fn a_new_stream_is_asked_from_its_first_frame_even_inside_the_interval() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    follow.answer(setting(4_000.0, 0.0));
    follow.rig.send(CameraCommand::StopStreaming);
    follow.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    follow.frames(1);
    assert!(follow.request().is_some(), "ответ прежнего потока новому не указ");
}

#[test]
fn the_end_of_the_stream_drops_the_question_and_a_late_answer_is_foreign() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let old = follow.request().expect("вопрос").id;
    follow.rig.send(CameraCommand::StopStreaming);
    assert_eq!(follow.request(), None, "вопрос ушедшему потоку снят");
    follow
        .rig
        .deliver_answer(&follow.outside, ready(old.0, setting(9_000.0, 0.0)));
    assert_eq!(follow.rig.held(), phone(setting(4_000.0, 0.0)));
    follow.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    // Часы нового потока могут начаться раньше: темп не обязан ждать, пока они догонят.
    follow.next_frame = 1;
    follow.frames(1);
    assert!(follow.request().is_some_and(|request| request.id > old));
}

#[test]
fn returning_with_auto_hides_the_first_frames_until_the_answer_under_either_rule() {
    for rule in [WhiteResumeRule::TakeFromSource, WhiteResumeRule::KeepLast] {
        let mut follow = FollowRig::remembering(rule, remembered_source(3_700.0), followed());
        assert_eq!(follow.state(), FOLLOWING, "{rule:?}: режим запомнен");
        follow.rig.send(CameraCommand::EnterForeground);
        follow.rig.start_streaming_on_wide();
        follow.frames(1);
        let request = follow.request().expect("открытие спросило");
        assert_eq!(request.while_awaiting, WhileAwaitingWhite::HideFrames, "{rule:?}");
        assert!(matches!(
            follow.rig.white_status().request,
            WhiteRequestState::Waiting { .. }
        ));
        // Мёртвая зона открытия не касается: ответ встаёт точно.
        follow.answer(setting(3_710.0, 0.0));
        assert_eq!(follow.rig.held(), phone(setting(3_710.0, 0.0)), "{rule:?}");
        follow.round(setting(5_600.0, 0.0));
        assert_eq!(
            follow.rig.held(),
            phone(setting(5_600.0, 0.0)),
            "{rule:?}: следит дальше"
        );
    }
}

#[test]
fn the_pace_never_replaces_the_opening_request() {
    let mut follow = FollowRig::remembering(WhiteResumeRule::TakeFromSource, remembered_source(3_700.0), followed());
    follow.rig.send(CameraCommand::EnterForeground);
    follow.rig.start_streaming_on_wide();
    follow.frames(1);
    let opening = follow.request().expect("открытие").id;
    follow.frames(ASK_EVERY_FRAMES * 4);
    assert_eq!(follow.request().map(|request| request.id), Some(opening));
    assert_eq!(follow.asked(), 0);
}

#[test]
fn a_manual_white_remembered_with_auto_on_is_still_replaced_by_the_phone() {
    // Auto нажали на ручной белой и приложение убили раньше ответа.
    let remembered = WhiteMemoryLoad::Remembered(WhiteMemoryRecord {
        setting: setting(5_600.0, 0.0),
        origin: RememberedOrigin::Manual,
    });
    let mut follow = FollowRig::remembering(WhiteResumeRule::KeepLast, remembered, followed());
    follow.rig.send(CameraCommand::EnterForeground);
    follow.rig.start_streaming_on_wide();
    follow.frames(1);
    follow.answer(setting(3_000.0, 0.0));
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));
}

#[test]
fn a_lock_that_survived_the_background_holds_the_white_and_hides_nothing() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.lock();
    follow.rig.send(CameraCommand::EnterBackground);
    let generation = follow.outside.generation();
    follow.rig.send(CameraCommand::EnterForeground);
    follow.frames(60);
    assert_eq!(
        follow.outside.generation(),
        generation,
        "ни заявки, ни новой белой (WB55)"
    );
    assert_eq!(follow.rig.held(), phone(setting(3_000.0, 0.0)));
    follow.unlock();
    follow.frames(1);
    follow.answer(setting(5_000.0, 0.0));
    assert_eq!(follow.rig.held(), phone(setting(5_000.0, 0.0)));
}

#[test]
fn the_background_drops_the_question_silently() {
    let mut follow = FollowRig::following();
    follow.frames(1);
    let asked = follow.request().expect("вопрос").id.0;
    follow.rig.send(CameraCommand::EnterBackground);
    assert_eq!(follow.request(), None);
    follow
        .rig
        .deliver_answer(&follow.outside, ready(asked, setting(9_000.0, 0.0)));
    assert_eq!(follow.rig.held(), phone(setting(4_000.0, 0.0)));
    assert!(finished_requests(&follow.rig.fresh_events()).is_empty());
    assert_eq!(follow.state(), FOLLOWING, "фон режима не выключает");
}

#[test]
fn auto_works_before_the_stream_and_is_remembered_at_once() {
    let mut follow = FollowRig::over(white_rig());
    assert_eq!(follow.follow(), CommandOutcome::Accepted);
    assert_eq!(follow.outside.saved_follows(), vec![followed()]);
    assert_eq!(
        follow_changes(&follow.rig.fresh_events()),
        vec![WhiteKeeping::FollowSource]
    );
    assert_eq!(follow.request(), None);
}

#[test]
fn going_to_the_background_flushes_a_fresh_choice_of_auto() {
    let mut follow = FollowRig::over(white_rig());
    follow.rig.send(CameraCommand::EnterForeground);
    let flushes = follow.outside.flushes();
    follow.follow();
    follow.rig.send(CameraCommand::EnterBackground);
    assert_eq!(
        follow.outside.flushes(),
        flushes + 1,
        "процесс в фоне могут убить: выбор на диске сразу"
    );
}

#[test]
fn moves_of_the_follow_are_saved_once_when_the_app_leaves_or_auto_is_turned_off() {
    let mut follow = FollowRig::following();
    let saved_before = follow.outside.saved().len();
    for kelvin in [3_000.0, 5_000.0, 6_500.0] {
        follow.round(setting(kelvin, 0.0));
    }
    assert_eq!(
        follow.outside.saved().len(),
        saved_before,
        "переезды на диск не идут: их бывает по десять в секунду"
    );

    let flushes = follow.outside.flushes();
    follow.rig.send(CameraCommand::EnterBackground);
    let last = WhiteMemoryRecord {
        setting: setting(6_500.0, 0.0),
        origin: RememberedOrigin::Source,
    };
    assert_eq!(
        follow.outside.saved()[saved_before..],
        [last],
        "ушли в фон — запомнена последняя"
    );
    assert_eq!(follow.outside.flushes(), flushes + 1, "и сброшена на диск");

    // Выключили «взять разово», а источник отказал: белая слежения осталась действующей и обязана помниться.
    follow.rig.send(CameraCommand::EnterForeground);
    follow.frames(1);
    follow.answer(setting(6_500.0, 0.0));
    let saved_before = follow.outside.saved().len();
    follow.round(setting(3_000.0, 0.0));
    follow.rig.send(CameraCommand::TakeWhite {
        source: PHONE,
        transition: WhiteTransition::Immediate,
    });
    follow.refuse(WhiteSourceOutcome::Busy);
    assert_eq!(
        follow.outside.saved()[saved_before..],
        [WhiteMemoryRecord {
            setting: setting(3_000.0, 0.0),
            origin: RememberedOrigin::Source,
        }]
    );
}

#[test]
fn a_hand_white_under_auto_is_saved_instead_of_the_followed_one() {
    let mut follow = FollowRig::following();
    follow.round(setting(3_000.0, 0.0));
    follow.rig.send(CameraCommand::SetWhite {
        setting: setting(5_600.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    follow.rig.send(CameraCommand::EnterBackground);
    assert_eq!(
        follow.outside.saved().last(),
        Some(&WhiteMemoryRecord {
            setting: setting(5_600.0, 0.0),
            origin: RememberedOrigin::Manual,
        }),
        "белая слежения ручную не затёрла"
    );
}

#[test]
fn the_snapshot_before_the_engine_starts_already_shows_the_remembered_auto() {
    let (mut wiring, _outside) = white_wiring(remembered_source(3_700.0));
    wiring.remembered_follow = followed();
    let status = crate::engine::initial_white_status(&crate::white::WhiteConfig::default(), &wiring);
    assert_eq!(status.follow.state, FOLLOWING);
}

// ---- Скорость ----

#[test]
fn the_chosen_speed_is_remembered_and_used_from_the_next_move_only() {
    let mut follow = FollowRig::following();
    let generation = follow.outside.generation();
    let fast = WhiteFollowSpeed::from_millis(100).expect("0,1 с");
    assert_eq!(
        follow.rig.send(CameraCommand::SetWhiteFollowSpeed { speed: fast }),
        CommandOutcome::Accepted
    );
    assert_eq!(
        follow.rig.send(CameraCommand::SetWhiteFollowSpeed { speed: fast }),
        CommandOutcome::AlreadyInState
    );
    assert_eq!(
        follow.outside.generation(),
        generation,
        "сама скорость белую не трогает"
    );
    assert_eq!(
        follow.outside.saved_follows().last(),
        Some(&WhiteFollowRecord {
            keeping: WhiteKeeping::FollowSource,
            speed: Some(fast)
        })
    );
    follow.round(setting(5_600.0, 0.0));
    assert_eq!(
        follow.outside.directive().transition,
        WhiteTransition::Over(Duration::from_millis(100))
    );
    // Выключение режима выбранную скорость не забывает.
    follow.rig.send(CameraCommand::SetWhite {
        setting: setting(3_200.0, 0.0),
        transition: WhiteTransition::Immediate,
    });
    assert_eq!(follow.rig.white_status().follow.speed, fast);
    assert_eq!(
        follow.outside.saved_follows().last(),
        Some(&WhiteFollowRecord {
            keeping: WhiteKeeping::Hold,
            speed: Some(fast)
        })
    );
}

#[test]
fn a_remembered_speed_wins_over_the_config_and_does_not_turn_auto_on() {
    let record = WhiteFollowRecord {
        keeping: WhiteKeeping::Hold,
        speed: Some(WhiteFollowSpeed::from_millis(100).expect("0,1 с")),
    };
    let follow = FollowRig::remembering(WhiteResumeRule::KeepLast, remembered_source(3_700.0), record);
    let status = follow.rig.white_status().follow;
    assert_eq!(status.speed.millis(), 100);
    assert_eq!(status.state, WhiteFollowState::Off);
    assert!(
        follow.outside.saved_follows().is_empty(),
        "прочитанное заново не пишется"
    );
}

// ---- Вспышка ----

/// Слежение на стенде вспышки.
fn flash_follow(flash: &FlashRig) -> crate::white::WhiteFollowStatus {
    flash.rig.engine.status().three_a.white.follow
}

#[test]
fn the_source_is_not_asked_during_a_flash_shot_and_a_new_stream_is_free_of_its_trace() {
    let mut flash = FlashRig::measured(8.0);
    assert_eq!(flash.capture(SHOT), CommandOutcome::Accepted);
    flash.rig.send(CameraCommand::FollowWhite { source: PHONE });
    for _ in 0..ASK_EVERY_FRAMES * 2 {
        flash.feed_repeating();
    }
    assert_ne!(flash.phase(), FlashPhase::Idle, "снимок идёт");
    assert_eq!(flash_follow(&flash).asked, 0, "свет сцены подмешан светодиодом");

    // Снимок сорвался концом потока: новый поток вспышки не видел и спрашивается сразу.
    flash.rig.send(CameraCommand::StopStreaming);
    flash.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    assert_eq!(flash.phase(), FlashPhase::Idle);
    flash.feed_repeating();
    assert_eq!(flash_follow(&flash).asked, 1);
}

#[test]
fn a_question_pending_when_the_flash_starts_is_dropped_and_after_the_shot_the_phone_settles_first() {
    let mut flash = FlashRig::measured(8.0);
    flash.rig.send(CameraCommand::FollowWhite { source: PHONE });
    flash.feed_repeating();
    assert_eq!(
        flash_follow(&flash).asked,
        1,
        "без вспышки первый кадр спрашивает; ответа нет — вопрос висит"
    );

    flash.capture(SHOT);
    flash.feed_repeating();
    // Снимок сорвался по сроку предвспышки: поток жив, а фонарик автоматика телефона видела.
    flash.rig.advance(ms(500));
    assert_eq!(flash.phase(), FlashPhase::Idle);
    // Секунда успокоения — 60 кадров после первого обычного.
    for _ in 0..60 {
        flash.feed_repeating();
        assert_eq!(flash_follow(&flash).asked, 1, "автоматика ещё помнит вспышку");
    }
    flash.feed_repeating();
    assert_eq!(
        flash_follow(&flash).asked,
        2,
        "успокоилась — спрашиваем; прежний вопрос снят (иначе ждали бы его 3 с)"
    );
}
