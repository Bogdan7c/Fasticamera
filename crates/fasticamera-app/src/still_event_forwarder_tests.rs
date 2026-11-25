//! Тесты пересылки событий рендера в службу: переполнение очереди (учёт, итог по сроку, без двойного итога
//! экрану) и остановленная служба.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use std::sync::Arc;

use camera_contract::geometry::PixelSize;
use camera_screen::ScreenAction;
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId};
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::StillId;
use still_contract::thumbnail::StillThumbnail;

use crate::camera_screen_capture::CaptureCompletionSlot;
use crate::camera_screen_input::ScreenInput;
use crate::photo_config::PhotoConfig;
use crate::still_event_forwarder::DevelopEventForwarder;
use crate::still_screen_ledger::{StillScreenLedger, StillThumbnailSink};
use crate::still_test_support::{RenderMode, StillRig, facts_for, shot, wait_until};
use photo_output::service::DevelopSubmitError;
use still_contract::develop::DevelopEvent;

/// Таблица без экрана: тестам пересылки событий она нужна только как приёмник миниатюр.
fn standalone_ledger() -> Arc<StillScreenLedger> {
    Arc::new(StillScreenLedger::new(Arc::new(CaptureCompletionSlot::default()), 8))
}

fn forwarder_for(rig: &StillRig, ledger: &Arc<StillScreenLedger>) -> DevelopEventForwarder {
    DevelopEventForwarder::new(rig.service.handle(), Arc::clone(ledger) as Arc<dyn StillThumbnailSink>)
}

fn finished_camera(request: u64) -> ScreenInput {
    ScreenInput::action(ScreenAction::CaptureFinished {
        request: ShotRequestId(request),
        outcome: CaptureOutcome::Failed(CaptureFailureReason::Camera),
    })
}

fn finished_failed(request: u64) -> ScreenInput {
    ScreenInput::action(ScreenAction::CaptureFinished {
        request: ShotRequestId(request),
        outcome: CaptureOutcome::Failed(CaptureFailureReason::Processing),
    })
}

#[test]
fn full_service_queue_loses_the_event_counts_it_and_the_term_ends_the_still_exactly_once() {
    // Короткий срок захвата: потерянное событие «кадр не захвачен» должно дать итог по сроку быстро.
    let mut config = PhotoConfig::default();
    config.service.event_queue_capacity = 1;
    config.service.capture_timeout_ms = 300;
    let mut rig = StillRig::start_with(config, RenderMode::Accepting);
    for request in 1..=3 {
        rig.capture.submit(
            shot(request),
            crate::look_test_support::neutral_recipe(),
            MonotonicNanos::from_nanos(0),
        );
    }
    // Рабочий поток службы застревает в платформенном вызове (создание черновика) — очередь событий не разбирается.
    rig.store.block_creation();
    rig.render.emit(
        StillId(1),
        DevelopEvent::Secured {
            facts: facts_for(StillId(1)),
            raw: None,
        },
    );
    wait_until(
        "служба взяла событие и застряла в платформе",
        || rig.store.creation_attempts() == 1,
    );
    // Очередь на одно место: второе событие встаёт в неё, третье — отвергнуто.
    rig.render.emit(
        StillId(2),
        DevelopEvent::NotSecured {
            id: StillId(2),
            reason: NoFrameReason::RingEmpty,
        },
    );
    rig.render.emit(
        StillId(3),
        DevelopEvent::NotSecured {
            id: StillId(3),
            reason: NoFrameReason::RingEmpty,
        },
    );
    let real = rig.backend.diagnostics().real.expect("настоящий режим");
    assert_eq!(real.events.forwarded, 2);
    assert_eq!(real.events.rejected_queue_full, 1, "переполнение посчитано приложением");
    assert_eq!(real.service.rejected_events, 1, "и самой службой");

    rig.store.unblock_creation();

    // Третий снимок итога от рендера не получил: его закрывает срок захвата — одним `CaptureFinished`.
    wait_until("итог снимка №3 по сроку", || {
        rig.service.stats().timed_out >= 1
    });
    let mut all = Vec::new();
    wait_until("экран получил итог снимка №3", || {
        all.extend(rig.screen_inputs());
        all.contains(&finished_failed(3))
    });
    // Запоздавшее событие рендера для уже законченного снимка игнорируется и не создаёт второго итога.
    rig.render.emit(
        StillId(3),
        DevelopEvent::NotSecured {
            id: StillId(3),
            reason: NoFrameReason::RingEmpty,
        },
    );
    wait_until(
        "опоздавшее событие учтено службой",
        || rig.service.stats().late_events >= 1,
    );
    all.extend(rig.screen_inputs());
    let third_finished = all.iter().filter(|input| **input == finished_failed(3)).count();
    assert_eq!(third_finished, 1, "итог снимка №3 доставлен ровно один раз: {all:?}");
}

#[test]
fn stopped_service_rejects_events_with_its_own_counter() {
    let rig = StillRig::start();
    let forwarder = forwarder_for(&rig, &standalone_ledger());
    assert_eq!(rig.service.shutdown(), photo_output::service::ShutdownReport::Stopped);

    let result = forwarder.forward(DevelopEvent::NotSecured {
        id: StillId(1),
        reason: NoFrameReason::RingEmpty,
    });

    assert_eq!(result, Err(DevelopSubmitError::ServiceStopped));
    let counters = forwarder.counters();
    assert_eq!((counters.forwarded, counters.rejected_service_stopped), (0, 1));
}

#[test]
fn accepted_events_are_counted_and_reach_the_service() {
    let mut rig = StillRig::start();
    rig.capture.submit(
        shot(1),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );
    rig.render.emit(
        StillId(1),
        DevelopEvent::NotSecured {
            id: StillId(1),
            reason: NoFrameReason::RingEmpty,
        },
    );
    // Событие дошло до службы: снимок закончился и экран получил итог.
    assert_eq!(rig.wait_screen_inputs(1), vec![finished_camera(1)]);
    let real = rig.backend.diagnostics().real.expect("настоящий режим");
    assert_eq!(real.events.forwarded, 1);
    assert_eq!(real.events.rejected_queue_full, 0);
}

#[test]
fn secured_event_leaves_its_frame_choice_in_the_log_even_when_the_service_rejects_it() {
    let rig = StillRig::start();
    let forwarder = forwarder_for(&rig, &standalone_ledger());
    assert_eq!(rig.service.shutdown(), photo_output::service::ShutdownReport::Stopped);
    let facts = facts_for(StillId(5));
    let expected = facts.choice;

    let result = forwarder.forward(DevelopEvent::Secured { facts, raw: None });

    assert_eq!(result, Err(DevelopSubmitError::ServiceStopped));
    assert_eq!(forwarder.choice_log().choice_of(StillId(5)), Some(expected));
}

#[test]
fn events_without_a_chosen_frame_leave_the_choice_log_untouched() {
    let rig = StillRig::start();
    let forwarder = forwarder_for(&rig, &standalone_ledger());

    let _ = forwarder.forward(DevelopEvent::NotSecured {
        id: StillId(6),
        reason: NoFrameReason::RingEmpty,
    });

    assert!(forwarder.choice_log().snapshot().is_empty());
}

#[test]
fn a_thumbnail_from_the_render_goes_to_the_ledger_and_not_to_the_service() {
    let rig = StillRig::start();
    let ledger = standalone_ledger();
    let forwarder = forwarder_for(&rig, &ledger);
    ledger.register(StillId(4), ShotRequestId(1), ledger.current_screen_epoch());
    let thumbnail = StillThumbnail::new(StillId(4), PixelSize::new(2, 1), vec![5; 8]).unwrap();

    render_loop::still::StillEventSink::deliver_thumbnail(&forwarder, thumbnail);

    let ledger_counters = ledger.counters();
    assert_eq!(
        ledger_counters.thumbnails_held, 1,
        "миниатюра у учёта снимков: {ledger_counters:?}"
    );
    assert_eq!(forwarder.counters().forwarded, 0, "служба записи миниатюру не получает");
}
