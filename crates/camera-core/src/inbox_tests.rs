//! Тесты почтового ящика: приоритеты, пределы очередей, вытеснение, пробуждение и остановка.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::CaptureIntent;

use super::*;
use crate::backend::{DeviceGeneration, SessionGeneration};

fn queues(command: usize, capture: usize) -> QueueConfig {
    QueueConfig {
        command_capacity: command,
        capture_event_capacity: capture,
        outward_event_capacity: 8,
    }
}

fn completed(frame: i64) -> BackendEvent {
    BackendEvent::CaptureCompleted {
        session: SessionGeneration(1),
        result: CaptureResultMeta {
            frame_number: FrameNumber(frame),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(frame * 1_000)),
            exposure_time: None,
            sensitivity_iso: None,
            reported_frame_duration: None,
            rolling_shutter_skew: None,
        },
    }
}

fn frame_of(item: &InboxItem) -> i64 {
    match item {
        InboxItem::BackendEvent(BackendEvent::CaptureCompleted { result, .. }) => result.frame_number.0,
        other => panic!("ожидался результат кадра, а пришло {other:?}"),
    }
}

const SOON: Option<Instant> = None;

#[test]
fn command_queue_rejects_when_full_and_frees_space_after_the_thread_takes_one() {
    let inbox = CameraInbox::new(queues(2, 4));
    let first = inbox.submit_command(CameraCommand::Close).expect("место есть");
    let second = inbox.submit_command(CameraCommand::StopStreaming).expect("место есть");
    assert_eq!((first, second), (CommandId(1), CommandId(2)));
    assert_eq!(
        inbox.submit_command(CameraCommand::Close),
        Err(CommandRejected::QueueFull)
    );
    assert_eq!(inbox.pending_commands(), 2, "отказанная команда в очередь не попала");

    assert_eq!(
        inbox.wait_next(SOON),
        InboxItem::Command {
            id: first,
            command: CameraCommand::Close
        }
    );
    let third = inbox.submit_command(CameraCommand::Close).expect("место освободилось");
    assert_eq!(third, CommandId(3), "номера не повторяются");
}

#[test]
fn capture_overflow_drops_the_oldest_and_counts_them() {
    let inbox = CameraInbox::new(queues(4, 3));
    for frame in 0..10 {
        inbox.deliver(completed(frame));
    }
    assert_eq!(inbox.dropped_capture_events(), 7);
    let kept: Vec<i64> = (0..3).map(|_| frame_of(&inbox.wait_next(SOON))).collect();
    assert_eq!(kept, vec![7, 8, 9], "остались самые свежие, по порядку");
}

#[test]
fn rare_events_are_never_dropped_and_are_served_before_commands_and_captures() {
    let inbox = CameraInbox::new(queues(4, 2));
    inbox.deliver(completed(1));
    inbox.submit_command(CameraCommand::Close).expect("место есть");
    // 1000 кадров затопили ящик, а редкие события лежат среди них.
    for frame in 0..1000 {
        inbox.deliver(completed(frame));
        if frame % 250 == 0 {
            inbox.deliver(BackendEvent::DeviceDisconnected {
                device: DeviceGeneration(frame as u64),
            });
        }
    }
    let mut disconnects = Vec::new();
    for _ in 0..4 {
        match inbox.wait_next(SOON) {
            InboxItem::BackendEvent(BackendEvent::DeviceDisconnected { device }) => disconnects.push(device.0),
            other => panic!("первыми должны идти редкие события, а пришло {other:?}"),
        }
    }
    assert_eq!(
        disconnects,
        vec![0, 250, 500, 750],
        "ни одно не потеряно, порядок сохранён"
    );
    assert!(
        matches!(inbox.wait_next(SOON), InboxItem::Command { .. }),
        "потом команды"
    );
    assert!(matches!(
        inbox.wait_next(SOON),
        InboxItem::BackendEvent(BackendEvent::CaptureCompleted { .. })
    ));
    assert_eq!(inbox.dropped_capture_events(), 1001 - 2);
}

#[test]
fn stop_request_wins_over_everything_and_is_idempotent() {
    let inbox = CameraInbox::new(queues(4, 4));
    inbox.deliver(BackendEvent::DeviceDisconnected {
        device: DeviceGeneration(1),
    });
    inbox.submit_command(CameraCommand::Close).expect("место есть");
    inbox.request_stop();
    inbox.request_stop();
    assert_eq!(inbox.wait_next(SOON), InboxItem::Stop);
    assert_eq!(inbox.wait_next(SOON), InboxItem::Stop);
}

#[test]
fn command_submitted_after_stop_request_is_rejected_not_silently_accepted() {
    let inbox = CameraInbox::new(queues(4, 4));
    inbox.request_stop();
    // Окно между `request_stop` и `mark_stopped`: поток команд уже не читает.
    assert_eq!(
        inbox.submit_command(CameraCommand::Close),
        Err(CommandRejected::EngineStopped)
    );
    assert_eq!(inbox.pending_commands(), 0, "команда в очередь не попала");
}

#[test]
fn commands_queued_before_stop_are_counted_as_discarded() {
    let inbox = CameraInbox::new(queues(4, 4));
    inbox.submit_command(CameraCommand::Close).expect("место есть");
    inbox.submit_command(CameraCommand::StopStreaming).expect("место есть");
    inbox.request_stop();
    inbox.mark_stopped();
    assert_eq!(inbox.commands_discarded_at_stop(), 2);
    assert_eq!(inbox.pending_commands(), 0);
}

#[test]
fn after_mark_stopped_commands_are_refused_and_events_are_discarded() {
    let inbox = CameraInbox::new(queues(4, 4));
    inbox.submit_command(CameraCommand::Close).expect("место есть");
    inbox.mark_stopped();
    assert_eq!(
        inbox.submit_command(CameraCommand::Close),
        Err(CommandRejected::EngineStopped)
    );
    inbox.deliver(completed(1));
    assert_eq!(inbox.pending_commands(), 0, "ожидавшие команды сброшены");
    assert_eq!(inbox.dropped_capture_events(), 0);
}

#[test]
fn empty_inbox_times_out_at_the_deadline_and_not_earlier() {
    let inbox = CameraInbox::new(queues(4, 4));
    let start = Instant::now();
    let item = inbox.wait_next(Some(start + Duration::from_millis(40)));
    assert_eq!(item, InboxItem::Timeout);
    assert!(start.elapsed() >= Duration::from_millis(40), "не раньше срока");
    assert!(start.elapsed() < Duration::from_secs(5), "и не зависает");
}

#[test]
fn due_timer_is_not_starved_by_a_flood_of_capture_events() {
    let inbox = CameraInbox::new(queues(4, 8));
    for frame in 0..8 {
        inbox.deliver(completed(frame));
    }
    let overdue = Instant::now() - Duration::from_millis(1);
    assert_eq!(inbox.wait_next(Some(overdue)), InboxItem::Timeout);
    // Кадры никуда не делись.
    assert_eq!(frame_of(&inbox.wait_next(SOON)), 0);
}

#[test]
fn blocked_waiter_wakes_up_when_another_thread_delivers() {
    let inbox = Arc::new(CameraInbox::new(queues(4, 4)));
    let producer = Arc::clone(&inbox);
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        producer.deliver(BackendEvent::DeviceDisconnected {
            device: DeviceGeneration(5),
        });
    });
    let item = inbox.wait_next(Some(Instant::now() + Duration::from_secs(10)));
    sender.join().expect("поток отправителя");
    assert_eq!(
        item,
        InboxItem::BackendEvent(BackendEvent::DeviceDisconnected {
            device: DeviceGeneration(5)
        })
    );
}

#[test]
fn command_from_another_thread_wakes_the_waiter_with_its_id() {
    let inbox = Arc::new(CameraInbox::new(queues(4, 4)));
    let producer = Arc::clone(&inbox);
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        producer.submit_command(CameraCommand::StartStreaming {
            intent: CaptureIntent::Preview,
        })
    });
    let item = inbox.wait_next(Some(Instant::now() + Duration::from_secs(10)));
    let id = sender.join().expect("поток отправителя").expect("принято");
    assert_eq!(
        item,
        InboxItem::Command {
            id,
            command: CameraCommand::StartStreaming {
                intent: CaptureIntent::Preview
            }
        }
    );
}
