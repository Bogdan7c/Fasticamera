//! Тесты фейк-сервиса снимка: журнал, два ответа (`CaptureSecured`, затем `CaptureFinished(Saved)`) в канал входа
//! экрана (сразу и с задержкой), отсутствие экрана, переполнение канала, снятие ответов старого экрана.

use camera_contract::ids::LensFacing;
use camera_contract::zoom::{ZoomCatalog, ZoomFraming, ZoomRatio};
use camera_screen::settings::{AspectRatio, FlashMode, RawCapture};
use camera_screen::shutter::{PrimaryFormat, ShotRequestId};
use shell_contract::orientation::DeviceOrientation;

use super::*;
use crate::camera_screen_input::{ScreenInput, screen_input_channel};

fn shot(id: u64) -> ShotRequest {
    ShotRequest {
        request: ShotRequestId(id),
        pressed_at: at_ms(0),
        framing: ZoomFraming {
            aspect: AspectRatio::FourByThree.crop_aspect(),
            facing: LensFacing::Back,
            ratio: ZoomRatio::ONE,
            catalog: Arc::new(ZoomCatalog::default()),
        },
        orientation: DeviceOrientation::Portrait,
        primary_format: PrimaryFormat::Heif,
        flash: FlashMode::Off,
        raw: RawCapture::Off,
        aspect: AspectRatio::FourByThree,
    }
}

fn at_ms(ms: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(ms * 1_000_000)
}

/// Запрос в момент 0 (для тестов с нулевой задержкой время не важно).
fn submit_at_zero(service: &FakeCaptureService, request: ShotRequest) -> SubmitOutcome {
    service.submit(request, crate::look_test_support::neutral_recipe(), at_ms(0))
}

fn secured(id: u64) -> ScreenInput {
    ScreenInput::Action(ScreenAction::CaptureSecured {
        request: ShotRequestId(id),
    })
}

/// Сроки без отдельной фазы захвата: `Secured` приходит вместе с `Saved` (после той же задержки).
fn saved_after(delay: Duration) -> FakeCaptureTiming {
    FakeCaptureTiming {
        secured_after: delay,
        saved_after: delay,
    }
}

/// Всё, что экран получил к этому моменту, по порядку.
fn drain(receiver: &mut crate::camera_screen_input::ScreenInputReceiver) -> Vec<ScreenInput> {
    std::iter::from_fn(|| receiver.try_next()).collect()
}

fn saved(id: u64) -> ScreenInput {
    ScreenInput::Action(ScreenAction::CaptureFinished {
        request: ShotRequestId(id),
        outcome: CaptureOutcome::Saved,
    })
}

#[test]
fn zero_delay_answers_saved_into_the_attached_screen_channel_and_journals_the_request() {
    let (sender, mut receiver) = screen_input_channel(4);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(saved_after(Duration::ZERO), 8, Arc::clone(&slot));

    assert_eq!(submit_at_zero(&service, shot(5)), SubmitOutcome::Accepted);

    assert_eq!(drain(&mut receiver), vec![secured(5), saved(5)]);
    assert_eq!(service.journal(), vec![shot(5)]);
    let counters = service.counters();
    assert_eq!((counters.submitted, counters.completions_delivered), (1, 1));
}

#[test]
fn delayed_answer_arrives_only_when_the_frame_clock_passes_the_delay_and_spawns_no_thread() {
    let (sender, mut receiver) = screen_input_channel(4);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(saved_after(Duration::from_millis(120)), 8, Arc::clone(&slot));

    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(1_000)),
        SubmitOutcome::Accepted
    );
    assert_eq!(service.journal(), vec![shot(1)], "запрос в журнале сразу");
    // Часы кадров не дошли до срока (1000 + 120): ответа нет, сколько бы реального времени ни прошло.
    service.poll_completions(at_ms(1_119));
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        drain(&mut receiver),
        Vec::<ScreenInput>::new(),
        "раньше срока ответа нет"
    );

    service.poll_completions(at_ms(1_120));
    assert_eq!(drain(&mut receiver), vec![secured(1), saved(1)], "в срок ответ пришёл");
    // Ответ выдаётся один раз.
    service.poll_completions(at_ms(5_000));
    assert_eq!(drain(&mut receiver), Vec::<ScreenInput>::new());
    let counters = service.counters();
    assert_eq!((counters.submitted, counters.completions_delivered), (1, 1));
}

#[test]
fn several_delayed_requests_are_answered_in_order_as_their_deadlines_pass() {
    let (sender, mut receiver) = screen_input_channel(8);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(saved_after(Duration::from_millis(100)), 8, Arc::clone(&slot));
    service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0));
    service.submit(shot(2), crate::look_test_support::neutral_recipe(), at_ms(50));

    service.poll_completions(at_ms(100));
    assert_eq!(drain(&mut receiver), vec![secured(1), saved(1)]);
    assert_eq!(drain(&mut receiver), Vec::<ScreenInput>::new(), "второй ещё не созрел");
    service.poll_completions(at_ms(150));
    assert_eq!(drain(&mut receiver), vec![secured(2), saved(2)]);
}

#[test]
fn the_journal_is_bounded_and_counts_evicted_entries() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::ZERO), 3, Arc::clone(&slot));
    for id in 1..=5 {
        assert_eq!(
            service.submit(shot(id), crate::look_test_support::neutral_recipe(), at_ms(0)),
            SubmitOutcome::Accepted
        );
    }
    assert_eq!(
        service.journal(),
        vec![shot(3), shot(4), shot(5)],
        "остались последние три"
    );
    let counters = service.counters();
    assert_eq!((counters.submitted, counters.journal_evicted), (5, 2));
}

#[test]
fn a_full_pending_queue_rejects_the_request_as_busy_without_journaling_it() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::from_millis(100)), 2, Arc::clone(&slot));
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Accepted
    );
    assert_eq!(
        service.submit(shot(2), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Accepted
    );
    assert_eq!(
        service.submit(shot(3), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Rejected(SubmitRejection::Busy)
    );
    assert_eq!(service.journal(), vec![shot(1), shot(2)]);
    assert_eq!(service.counters().rejected_pending_full, 1);
    // После ответов очередь освободилась.
    service.poll_completions(at_ms(100));
    assert_eq!(
        service.submit(shot(4), crate::look_test_support::neutral_recipe(), at_ms(100)),
        SubmitOutcome::Accepted
    );
}

#[test]
fn without_a_screen_the_answer_is_counted_not_swallowed() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::ZERO), 8, Arc::clone(&slot));

    assert_eq!(submit_at_zero(&service, shot(2)), SubmitOutcome::Accepted);

    let counters = service.counters();
    assert_eq!(counters.completions_without_screen, 2, "обе фазы: захват и сохранение");
    assert_eq!(counters.completions_delivered, 0);
    assert_eq!(service.journal().len(), 1, "запрос записан, даже если экрана нет");
}

#[test]
fn detach_stops_answers_and_reattach_retargets_to_the_new_screen() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::ZERO), 8, Arc::clone(&slot));
    let (first_sender, mut first_receiver) = screen_input_channel(4);
    slot.attach(first_sender.injector());
    slot.detach();
    submit_at_zero(&service, shot(1));
    assert_eq!(
        drain(&mut first_receiver),
        Vec::<ScreenInput>::new(),
        "снятому экрану ответ не идёт"
    );

    let (second_sender, mut second_receiver) = screen_input_channel(4);
    slot.attach(second_sender.injector());
    submit_at_zero(&service, shot(2));
    assert_eq!(
        drain(&mut second_receiver),
        vec![secured(2), saved(2)],
        "ответ идёт новому экрану"
    );
    assert_eq!(drain(&mut first_receiver), Vec::<ScreenInput>::new());
    assert_eq!(
        service.counters().completions_without_screen,
        2,
        "обе фазы первого снимка"
    );
}

#[test]
fn a_full_or_dead_screen_channel_counts_each_answer_as_lost() {
    // Канал на один ответ: `Secured` первого снимка занимает его, `Saved` уже не помещается.
    let (sender, receiver) = screen_input_channel(1);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(saved_after(Duration::ZERO), 8, Arc::clone(&slot));
    submit_at_zero(&service, shot(1));
    let counters = service.counters();
    assert_eq!((counters.secured_delivered, counters.completions_delivered), (1, 0));
    assert_eq!(
        counters.completions_lost, 1,
        "итог не поместился в очередь на один ответ"
    );
    drop(receiver);
    submit_at_zero(&service, shot(3));
    let counters = service.counters();
    assert_eq!(
        counters.completions_lost, 3,
        "оба ответа второго снимка ушли в мёртвый канал"
    );
    assert_eq!(counters.secured_delivered + counters.completions_delivered, 1);
}

#[test]
fn reply_owed_to_a_replaced_screen_is_dropped_and_does_not_complete_the_new_screens_shot_with_the_same_id() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::from_millis(100)), 8, Arc::clone(&slot));
    let (old_sender, mut old_receiver) = screen_input_channel(4);
    slot.attach(old_sender.injector());
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Accepted
    );

    // Экран переустановлен до ответа; новый экран нумерует снимки с 1 и снимает «свой» снимок id=1 позже.
    slot.detach();
    let (new_sender, mut new_receiver) = screen_input_channel(4);
    slot.attach(new_sender.injector());
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(50)),
        SubmitOutcome::Accepted
    );

    // Срок старого ответа (100) наступил, срок нового (150) — нет: новый экран не должен получить ничего.
    service.poll_completions(at_ms(120));
    assert_eq!(
        drain(&mut new_receiver),
        Vec::<ScreenInput>::new(),
        "ответ старого экрана закрыл снимок нового"
    );
    assert_eq!(
        drain(&mut old_receiver),
        Vec::<ScreenInput>::new(),
        "снятый экран ответа не получает"
    );
    // Собственный ответ нового экрана приходит в свой срок.
    service.poll_completions(at_ms(150));
    assert_eq!(drain(&mut new_receiver), vec![secured(1), saved(1)]);
    assert_eq!(
        drain(&mut new_receiver),
        Vec::<ScreenInput>::new(),
        "ровно один ответ на один снимок"
    );

    let counters = service.counters();
    assert_eq!(
        counters.completions_dropped_on_screen_change, 2,
        "обе фазы старого снимка"
    );
    assert_eq!(counters.completions_delivered, 1);
    assert_eq!(counters.secured_delivered, 1);
    assert_eq!(counters.submitted, 2);
    assert_eq!(
        service.journal(),
        vec![shot(1), shot(1)],
        "журнал переживает смену экрана"
    );
}

#[test]
fn stale_replies_do_not_occupy_the_queue_and_cause_a_false_busy_for_the_new_screen() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::from_millis(100)), 1, Arc::clone(&slot));
    let (old_sender, _old_receiver) = screen_input_channel(4);
    slot.attach(old_sender.injector());
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Accepted
    );

    let (new_sender, _new_receiver) = screen_input_channel(4);
    slot.attach(new_sender.injector());
    // Ёмкость очереди 1: без снятия чужого ответа новый снимок получил бы ложный `Busy`.
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(10)),
        SubmitOutcome::Accepted
    );
    let counters = service.counters();
    assert_eq!(counters.rejected_pending_full, 0);
    assert_eq!(
        counters.completions_dropped_on_screen_change, 2,
        "обе фазы старого снимка"
    );
}

#[test]
fn replies_of_the_current_screen_are_not_dropped_by_polling() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(saved_after(Duration::from_millis(100)), 8, Arc::clone(&slot));
    let (sender, mut receiver) = screen_input_channel(4);
    slot.attach(sender.injector());
    service.submit(shot(3), crate::look_test_support::neutral_recipe(), at_ms(0));
    service.poll_completions(at_ms(50));
    service.poll_completions(at_ms(100));
    assert_eq!(drain(&mut receiver), vec![secured(3), saved(3)]);
    assert_eq!(service.counters().completions_dropped_on_screen_change, 0);
}

#[test]
fn the_frame_is_secured_first_and_saved_later_each_on_its_own_deadline() {
    let (sender, mut receiver) = screen_input_channel(8);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let timing = FakeCaptureTiming {
        secured_after: Duration::from_millis(60),
        saved_after: Duration::from_millis(400),
    };
    let service = FakeCaptureService::new(timing, 8, Arc::clone(&slot));
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(1_000)),
        SubmitOutcome::Accepted
    );

    service.poll_completions(at_ms(1_059));
    assert_eq!(drain(&mut receiver), Vec::<ScreenInput>::new());
    service.poll_completions(at_ms(1_060));
    assert_eq!(
        drain(&mut receiver),
        vec![secured(1)],
        "через 60 мс — только «кадр захвачен»"
    );
    service.poll_completions(at_ms(1_399));
    assert_eq!(drain(&mut receiver), Vec::<ScreenInput>::new());
    service.poll_completions(at_ms(1_400));
    assert_eq!(drain(&mut receiver), vec![saved(1)], "через 400 мс — итог");
    let counters = service.counters();
    assert_eq!((counters.secured_delivered, counters.completions_delivered), (1, 1));
}

#[test]
fn the_second_shot_is_secured_while_the_first_is_still_saving_and_phases_do_not_block_each_other() {
    let (sender, mut receiver) = screen_input_channel(8);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let timing = FakeCaptureTiming {
        secured_after: Duration::from_millis(60),
        saved_after: Duration::from_millis(400),
    };
    let service = FakeCaptureService::new(timing, 8, Arc::clone(&slot));
    service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0));
    service.submit(shot(2), crate::look_test_support::neutral_recipe(), at_ms(100));

    // К 200 мс первый захвачен, второй захвачен (160), но ни один не сохранён: `Secured` второго не ждёт `Saved` первого.
    service.poll_completions(at_ms(200));
    assert_eq!(drain(&mut receiver), vec![secured(1), secured(2)]);
    service.poll_completions(at_ms(600));
    assert_eq!(drain(&mut receiver), vec![saved(1), saved(2)]);
}

#[test]
fn the_capture_phase_never_lags_behind_the_saving_phase() {
    let (sender, mut receiver) = screen_input_channel(8);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    // Опечатка в конфиге: захват «позже» сохранения. Фейк зажимает срок захвата до срока сохранения.
    let timing = FakeCaptureTiming {
        secured_after: Duration::from_millis(500),
        saved_after: Duration::from_millis(100),
    };
    let service = FakeCaptureService::new(timing, 8, Arc::clone(&slot));
    service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0));

    service.poll_completions(at_ms(100));

    assert_eq!(
        drain(&mut receiver),
        vec![secured(1), saved(1)],
        "сначала захват, потом итог"
    );
}

#[test]
fn a_capture_with_zero_secured_delay_is_secured_at_once_and_saved_later() {
    let (sender, mut receiver) = screen_input_channel(8);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let timing = FakeCaptureTiming {
        secured_after: Duration::ZERO,
        saved_after: Duration::from_millis(50),
    };
    let service = FakeCaptureService::new(timing, 8, Arc::clone(&slot));

    service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0));
    assert_eq!(drain(&mut receiver), vec![secured(1)], "захват без задержки");
    service.poll_completions(at_ms(50));
    assert_eq!(drain(&mut receiver), vec![saved(1)]);
}

#[test]
fn busy_counts_shots_waiting_for_the_result_not_individual_answers() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let timing = FakeCaptureTiming {
        secured_after: Duration::from_millis(10),
        saved_after: Duration::from_millis(100),
    };
    let service = FakeCaptureService::new(timing, 2, Arc::clone(&slot));
    assert_eq!(
        service.submit(shot(1), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Accepted
    );
    assert_eq!(
        service.submit(shot(2), crate::look_test_support::neutral_recipe(), at_ms(0)),
        SubmitOutcome::Accepted
    );
    // Захваты прошли, итогов ещё нет: оба снимка всё ещё ждут сохранения.
    service.poll_completions(at_ms(10));
    assert_eq!(
        service.submit(shot(3), crate::look_test_support::neutral_recipe(), at_ms(10)),
        SubmitOutcome::Rejected(SubmitRejection::Busy)
    );
    service.poll_completions(at_ms(100));
    assert_eq!(
        service.submit(shot(3), crate::look_test_support::neutral_recipe(), at_ms(100)),
        SubmitOutcome::Accepted
    );
}
