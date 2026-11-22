//! Переключатель сервиса снимка: запросы идут тому, кто подставлен; опрос ответов обходит всех, кому он нужен.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::camera_screen_capture::SubmitRejection;
use crate::still_test_support::shot;

/// Сервис, который помнит, сколько запросов и опросов получил, и отвечает заданным итогом.
struct CountingService {
    submits: AtomicU64,
    polls: AtomicU64,
    answer: SubmitOutcome,
}

impl CountingService {
    fn new(answer: SubmitOutcome) -> Arc<Self> {
        Arc::new(Self {
            submits: AtomicU64::new(0),
            polls: AtomicU64::new(0),
            answer,
        })
    }
}

impl CaptureService for CountingService {
    fn submit(&self, _request: ShotRequest, _now: MonotonicNanos) -> SubmitOutcome {
        self.submits.fetch_add(1, Ordering::SeqCst);
        self.answer
    }

    fn poll_completions(&self, _now: MonotonicNanos) {
        self.polls.fetch_add(1, Ordering::SeqCst);
    }
}

fn now() -> MonotonicNanos {
    MonotonicNanos::from_nanos(1)
}

#[test]
fn without_a_substitute_every_request_goes_to_the_primary_service_and_its_answer_is_returned() {
    let primary = CountingService::new(SubmitOutcome::Rejected(SubmitRejection::Busy));
    let switch = SwitchableCaptureService::new(Arc::clone(&primary) as Arc<dyn CaptureService>);

    let outcome = switch.submit(shot(1), now());

    assert_eq!(outcome, SubmitOutcome::Rejected(SubmitRejection::Busy));
    assert_eq!(primary.submits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_substitute_takes_the_requests_and_the_primary_gets_none_until_restore() {
    let primary = CountingService::new(SubmitOutcome::Accepted);
    let fake = CountingService::new(SubmitOutcome::Rejected(SubmitRejection::ServiceUnavailable));
    let switch = SwitchableCaptureService::new(Arc::clone(&primary) as Arc<dyn CaptureService>);

    switch.substitute(Arc::clone(&fake) as Arc<dyn CaptureService>);
    let during = switch.submit(shot(1), now());
    assert!(switch.restore(), "подстановка была");
    let after = switch.submit(shot(2), now());

    assert_eq!(during, SubmitOutcome::Rejected(SubmitRejection::ServiceUnavailable));
    assert_eq!(after, SubmitOutcome::Accepted);
    assert_eq!(
        (
            fake.submits.load(Ordering::SeqCst),
            primary.submits.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
}

#[test]
fn restore_without_a_substitute_changes_nothing_and_says_so() {
    let primary = CountingService::new(SubmitOutcome::Accepted);
    let switch = SwitchableCaptureService::new(Arc::clone(&primary) as Arc<dyn CaptureService>);

    assert!(!switch.restore());

    assert_eq!(switch.submit(shot(1), now()), SubmitOutcome::Accepted);
    assert_eq!(primary.submits.load(Ordering::SeqCst), 1);
}

#[test]
fn polling_reaches_the_primary_the_active_substitute_and_the_retired_one_still_owing_answers() {
    let primary = CountingService::new(SubmitOutcome::Accepted);
    let fake = CountingService::new(SubmitOutcome::Accepted);
    let switch = SwitchableCaptureService::new(Arc::clone(&primary) as Arc<dyn CaptureService>);

    switch.substitute(Arc::clone(&fake) as Arc<dyn CaptureService>);
    switch.poll_completions(now());
    assert_eq!(
        (primary.polls.load(Ordering::SeqCst), fake.polls.load(Ordering::SeqCst)),
        (1, 1)
    );

    // После возврата снятый фейк всё ещё опрашивается: снимок, принятый им до возврата, дождётся своего ответа.
    switch.restore();
    switch.poll_completions(now());
    assert_eq!(
        (primary.polls.load(Ordering::SeqCst), fake.polls.load(Ordering::SeqCst)),
        (2, 2)
    );
}
