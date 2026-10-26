//! Тесты ворот колбэка: закрытие ждёт вошедших, вошедшие после закрытия получают отказ.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::*;

const BOUND: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_millis(100);

#[test]
fn open_gate_admits_and_closed_gate_refuses() {
    let gate = CallbackGate::new();
    assert!(matches!(gate.admit(), GateAdmission::Admitted(_)));
    gate.close();
    gate.close(); // повторное закрытие безвредно
    assert!(matches!(gate.admit(), GateAdmission::Closed));
}

#[test]
fn close_waits_until_the_admitted_callback_leaves() {
    let gate = CallbackGate::new();
    let close_returned = AtomicBool::new(false);
    let (inside_tx, inside_rx) = mpsc::channel();
    let (leave_tx, leave_rx) = mpsc::channel::<()>();
    let leave_rx = std::sync::Mutex::new(leave_rx);

    thread::scope(|scope| {
        scope.spawn(|| {
            let GateAdmission::Admitted(_pass) = gate.admit() else {
                panic!("ворота открыты");
            };
            inside_tx.send(()).expect("главный поток ждёт");
            leave_rx
                .lock()
                .expect("замок цел")
                .recv_timeout(BOUND)
                .expect("главный поток разрешит выход");
        });
        inside_rx.recv_timeout(BOUND).expect("колбэк вошёл");

        let closer = scope.spawn(|| {
            gate.close();
            close_returned.store(true, Ordering::SeqCst);
        });
        thread::sleep(SETTLE);
        assert!(
            !close_returned.load(Ordering::SeqCst),
            "close вернулся, пока колбэк ещё внутри ворот"
        );

        leave_tx.send(()).expect("колбэк ждёт");
        closer.join().expect("close завершился");
        assert!(close_returned.load(Ordering::SeqCst));
    });
    assert!(matches!(gate.admit(), GateAdmission::Closed));
}

#[test]
fn panic_inside_the_gate_does_not_break_later_close() {
    let gate = CallbackGate::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _pass = gate.admit();
        panic!("паника колбэка");
    }));
    assert!(result.is_err());
    gate.close();
    assert!(matches!(gate.admit(), GateAdmission::Closed));
}
