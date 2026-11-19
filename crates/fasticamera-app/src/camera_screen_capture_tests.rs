//! Тесты фейк-сервиса снимка: журнал, ответ `Saved` в канал входа экрана (сразу и с задержкой), отсутствие экрана,
//! переполнение канала.

use camera_screen::settings::{AspectRatio, FlashMode, RawCapture};
use camera_screen::shutter::ShotRequestId;

use super::*;
use crate::camera_screen_input::{ScreenInput, screen_input_channel};

fn shot(id: u64) -> ShotRequest {
    ShotRequest {
        request: ShotRequestId(id),
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
    service.submit(request, at_ms(0))
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
    let service = FakeCaptureService::new(Duration::ZERO, 8, Arc::clone(&slot));

    assert_eq!(submit_at_zero(&service, shot(5)), SubmitOutcome::Accepted);

    assert_eq!(receiver.try_next(), Some(saved(5)));
    assert_eq!(service.journal(), vec![shot(5)]);
    let counters = service.counters();
    assert_eq!((counters.submitted, counters.completions_delivered), (1, 1));
}

#[test]
fn delayed_answer_arrives_only_when_the_frame_clock_passes_the_delay_and_spawns_no_thread() {
    let (sender, mut receiver) = screen_input_channel(4);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(Duration::from_millis(120), 8, Arc::clone(&slot));

    assert_eq!(service.submit(shot(1), at_ms(1_000)), SubmitOutcome::Accepted);
    assert_eq!(service.journal(), vec![shot(1)], "запрос в журнале сразу");
    // Часы кадров не дошли до срока (1000 + 120): ответа нет, сколько бы реального времени ни прошло.
    service.poll_completions(at_ms(1_119));
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(receiver.try_next(), None, "раньше срока ответа нет");

    service.poll_completions(at_ms(1_120));
    assert_eq!(receiver.try_next(), Some(saved(1)), "в срок ответ пришёл");
    // Ответ выдаётся один раз.
    service.poll_completions(at_ms(5_000));
    assert_eq!(receiver.try_next(), None);
    let counters = service.counters();
    assert_eq!((counters.submitted, counters.completions_delivered), (1, 1));
}

#[test]
fn several_delayed_requests_are_answered_in_order_as_their_deadlines_pass() {
    let (sender, mut receiver) = screen_input_channel(8);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(Duration::from_millis(100), 8, Arc::clone(&slot));
    service.submit(shot(1), at_ms(0));
    service.submit(shot(2), at_ms(50));

    service.poll_completions(at_ms(100));
    assert_eq!(receiver.try_next(), Some(saved(1)));
    assert_eq!(receiver.try_next(), None, "второй ещё не созрел");
    service.poll_completions(at_ms(150));
    assert_eq!(receiver.try_next(), Some(saved(2)));
}

#[test]
fn the_journal_is_bounded_and_counts_evicted_entries() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(Duration::ZERO, 3, Arc::clone(&slot));
    for id in 1..=5 {
        assert_eq!(service.submit(shot(id), at_ms(0)), SubmitOutcome::Accepted);
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
    let service = FakeCaptureService::new(Duration::from_millis(100), 2, Arc::clone(&slot));
    assert_eq!(service.submit(shot(1), at_ms(0)), SubmitOutcome::Accepted);
    assert_eq!(service.submit(shot(2), at_ms(0)), SubmitOutcome::Accepted);
    assert_eq!(
        service.submit(shot(3), at_ms(0)),
        SubmitOutcome::Rejected(SubmitRejection::Busy)
    );
    assert_eq!(service.journal(), vec![shot(1), shot(2)]);
    assert_eq!(service.counters().rejected_pending_full, 1);
    // После ответов очередь освободилась.
    service.poll_completions(at_ms(100));
    assert_eq!(service.submit(shot(4), at_ms(100)), SubmitOutcome::Accepted);
}

#[test]
fn without_a_screen_the_answer_is_counted_not_swallowed() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(Duration::ZERO, 8, Arc::clone(&slot));

    assert_eq!(submit_at_zero(&service, shot(2)), SubmitOutcome::Accepted);

    let counters = service.counters();
    assert_eq!(counters.completions_without_screen, 1);
    assert_eq!(counters.completions_delivered, 0);
    assert_eq!(service.journal().len(), 1, "запрос записан, даже если экрана нет");
}

#[test]
fn detach_stops_answers_and_reattach_retargets_to_the_new_screen() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(Duration::ZERO, 8, Arc::clone(&slot));
    let (first_sender, mut first_receiver) = screen_input_channel(4);
    slot.attach(first_sender.injector());
    slot.detach();
    submit_at_zero(&service, shot(1));
    assert_eq!(first_receiver.try_next(), None, "снятому экрану ответ не идёт");

    let (second_sender, mut second_receiver) = screen_input_channel(4);
    slot.attach(second_sender.injector());
    submit_at_zero(&service, shot(2));
    assert_eq!(second_receiver.try_next(), Some(saved(2)), "ответ идёт новому экрану");
    assert_eq!(first_receiver.try_next(), None);
    assert_eq!(service.counters().completions_without_screen, 1);
}

#[test]
fn a_full_or_dead_screen_channel_counts_the_answer_as_lost() {
    let (sender, receiver) = screen_input_channel(1);
    let slot = Arc::new(CaptureCompletionSlot::default());
    slot.attach(sender.injector());
    let service = FakeCaptureService::new(Duration::ZERO, 8, Arc::clone(&slot));
    submit_at_zero(&service, shot(1));
    submit_at_zero(&service, shot(2));
    assert_eq!(
        service.counters().completions_lost,
        1,
        "очередь на один ответ уже занята"
    );
    drop(receiver);
    submit_at_zero(&service, shot(3));
    let counters = service.counters();
    assert_eq!((counters.completions_delivered, counters.completions_lost), (1, 2));
}

#[test]
fn reply_owed_to_a_replaced_screen_is_dropped_and_does_not_complete_the_new_screens_shot_with_the_same_id() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(Duration::from_millis(100), 8, Arc::clone(&slot));
    let (old_sender, mut old_receiver) = screen_input_channel(4);
    slot.attach(old_sender.injector());
    assert_eq!(service.submit(shot(1), at_ms(0)), SubmitOutcome::Accepted);

    // Экран переустановлен до ответа; новый экран нумерует снимки с 1 и снимает «свой» снимок id=1 позже.
    slot.detach();
    let (new_sender, mut new_receiver) = screen_input_channel(4);
    slot.attach(new_sender.injector());
    assert_eq!(service.submit(shot(1), at_ms(50)), SubmitOutcome::Accepted);

    // Срок старого ответа (100) наступил, срок нового (150) — нет: новый экран не должен получить ничего.
    service.poll_completions(at_ms(120));
    assert_eq!(
        new_receiver.try_next(),
        None,
        "ответ старого экрана закрыл снимок нового"
    );
    assert_eq!(old_receiver.try_next(), None, "снятый экран ответа не получает");
    // Собственный ответ нового экрана приходит в свой срок.
    service.poll_completions(at_ms(150));
    assert_eq!(new_receiver.try_next(), Some(saved(1)));
    assert_eq!(new_receiver.try_next(), None, "ровно один ответ на один снимок");

    let counters = service.counters();
    assert_eq!(counters.completions_dropped_on_screen_change, 1);
    assert_eq!(counters.completions_delivered, 1);
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
    let service = FakeCaptureService::new(Duration::from_millis(100), 1, Arc::clone(&slot));
    let (old_sender, _old_receiver) = screen_input_channel(4);
    slot.attach(old_sender.injector());
    assert_eq!(service.submit(shot(1), at_ms(0)), SubmitOutcome::Accepted);

    let (new_sender, _new_receiver) = screen_input_channel(4);
    slot.attach(new_sender.injector());
    // Ёмкость очереди 1: без снятия чужого ответа новый снимок получил бы ложный `Busy`.
    assert_eq!(service.submit(shot(1), at_ms(10)), SubmitOutcome::Accepted);
    let counters = service.counters();
    assert_eq!(counters.rejected_pending_full, 0);
    assert_eq!(counters.completions_dropped_on_screen_change, 1);
}

#[test]
fn replies_of_the_current_screen_are_not_dropped_by_polling() {
    let slot = Arc::new(CaptureCompletionSlot::default());
    let service = FakeCaptureService::new(Duration::from_millis(100), 8, Arc::clone(&slot));
    let (sender, mut receiver) = screen_input_channel(4);
    slot.attach(sender.injector());
    service.submit(shot(3), at_ms(0));
    service.poll_completions(at_ms(50));
    service.poll_completions(at_ms(100));
    assert_eq!(receiver.try_next(), Some(saved(3)));
    assert_eq!(service.counters().completions_dropped_on_screen_change, 0);
}
