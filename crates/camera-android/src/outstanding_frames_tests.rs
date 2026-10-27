use std::sync::Arc;
use std::thread;

use super::*;

#[test]
fn count_follows_acquired_and_returned_frames() {
    let frames = OutstandingFrames::new();
    assert_eq!(frames.count(), 0);
    frames.frame_acquired();
    frames.frame_acquired();
    assert_eq!(frames.count(), 2);
    frames.frame_returned();
    assert_eq!(frames.count(), 1);
}

#[test]
fn extra_return_does_not_wrap_below_zero_so_the_reader_is_not_freed_early() {
    let frames = OutstandingFrames::new();
    frames.frame_returned();
    assert_eq!(frames.count(), 0);
    frames.frame_acquired();
    assert_eq!(frames.count(), 1, "лишний возврат не создал «долг»");
}

#[test]
fn counting_is_exact_under_concurrent_acquire_and_return() {
    let frames = Arc::new(OutstandingFrames::new());
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let shared = Arc::clone(&frames);
            thread::spawn(move || {
                for _ in 0..500 {
                    shared.frame_acquired();
                }
                for _ in 0..300 {
                    shared.frame_returned();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("поток счёта");
    }
    assert_eq!(frames.count(), 4 * 200);
}
