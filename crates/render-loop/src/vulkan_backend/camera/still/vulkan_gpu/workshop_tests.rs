//! Тесты мастерской снимка: работа идёт НЕ на вызывающем потоке, опрос не ждёт, у каждой работы ровно один
//! итог (результат или `NoResult`), падение одной работы не ломает следующие, остановка ждёт только начатую
//! работу и пропускает не начатое, а брошенный результат разбирается на потоке мастерской.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тесты")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use super::{StillWorkshop, TicketPoll, WorkshopError};

const EVENTUALLY: Duration = Duration::from_secs(5);

/// Опрашивает билет, пока не придёт итог.
fn poll_until_done<T>(ticket: &super::Ticket<T>) -> TicketPoll<T> {
    let deadline = Instant::now() + EVENTUALLY;
    loop {
        match ticket.poll() {
            TicketPoll::Pending => {
                assert!(
                    Instant::now() < deadline,
                    "работа мастерской не закончилась за {EVENTUALLY:?}"
                );
                thread::sleep(Duration::from_millis(1));
            }
            done => return done,
        }
    }
}

#[test]
fn work_runs_on_the_workshop_thread_and_its_result_comes_back() {
    let workshop = StillWorkshop::start().unwrap();
    let caller = thread::current().id();
    let ticket = workshop
        .submit("тест", move || {
            (thread::current().id(), thread::current().name().map(str::to_owned))
        })
        .unwrap();

    let TicketPoll::Ready((worker, name)) = poll_until_done(&ticket) else {
        panic!("ждали результат");
    };
    assert_ne!(worker, caller, "работа обязана идти не на потоке рендера");
    assert_eq!(name.as_deref(), Some("fasticamera-still-workshop"));
    // Итог ровно один: повторный опрос результата не даёт.
    assert!(matches!(ticket.poll(), TicketPoll::NoResult));
}

#[test]
fn poll_does_not_wait_for_slow_work() {
    let workshop = StillWorkshop::start().unwrap();
    let gate = Arc::new(Barrier::new(2));
    let worker_gate = Arc::clone(&gate);
    let ticket = workshop
        .submit("тест", move || {
            worker_gate.wait();
            7_u32
        })
        .unwrap();

    // Работа стоит на барьере: опрос обязан вернуться сразу с `Pending`.
    let started = Instant::now();
    assert!(matches!(ticket.poll(), TicketPoll::Pending));
    assert!(started.elapsed() < Duration::from_millis(50), "опрос ждал работу");
    gate.wait();
    assert!(matches!(poll_until_done(&ticket), TicketPoll::Ready(7)));
}

#[test]
fn a_panicking_job_reports_no_result_and_the_next_job_still_runs() {
    let workshop = StillWorkshop::start().unwrap();
    let failed = workshop
        .submit("тест", || -> u32 {
            panic!("работа упала нарочно")
        })
        .unwrap();
    let next = workshop.submit("тест", || 42_u32).unwrap();

    assert!(matches!(poll_until_done(&failed), TicketPoll::NoResult));
    assert!(matches!(poll_until_done(&next), TicketPoll::Ready(42)));
}

#[test]
fn jobs_run_one_at_a_time_in_submission_order() {
    let workshop = StillWorkshop::start().unwrap();
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tickets: Vec<_> = (0..5)
        .map(|index| {
            let order = Arc::clone(&order);
            workshop
                .submit("тест", move || order.lock().unwrap().push(index))
                .unwrap()
        })
        .collect();
    for ticket in &tickets {
        assert!(matches!(poll_until_done(ticket), TicketPoll::Ready(())));
    }
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3, 4]);
}

#[test]
fn drop_waits_only_for_started_work_skips_queued_work_and_disposes_the_orphaned_result() {
    let workshop = StillWorkshop::start().unwrap();
    let release = Arc::new(AtomicBool::new(false));
    let disposed = Arc::new(AtomicBool::new(false));
    let queued_ran = Arc::new(AtomicBool::new(false));
    let (worker_release, worker_disposed, worker_queued) =
        (Arc::clone(&release), Arc::clone(&disposed), Arc::clone(&queued_ran));
    let running = workshop
        .submit_or_dispose(
            "тест",
            move || {
                while !worker_release.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                "цель"
            },
            move |orphan| {
                assert_eq!(orphan, "цель");
                worker_disposed.store(true, Ordering::SeqCst);
            },
        )
        .unwrap();
    let queued = workshop
        .submit("тест", move || worker_queued.store(true, Ordering::SeqCst))
        .unwrap();
    // Дождаться, пока первая работа начнётся (иначе остановка пропустила бы и её).
    thread::sleep(Duration::from_millis(20));
    // Билет брошен до конца работы: результат уходит в `dispose` на потоке мастерской.
    drop(running);
    // Отпустить работу чуть позже, чем начнётся `Drop`: `Drop` обязан дождаться именно её.
    let releaser = {
        let release = Arc::clone(&release);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            release.store(true, Ordering::SeqCst);
        })
    };
    drop(workshop);
    releaser.join().unwrap();
    // К концу `Drop` начатая работа доделана и брошенный результат разобран; поток мастерской завершён.
    assert!(disposed.load(Ordering::SeqCst), "Drop вернулся раньше начатой работы");
    // Работа, не начатая к остановке, не выполняется: её билет видит `NoResult`.
    assert!(matches!(queued.poll(), TicketPoll::NoResult));
    assert!(
        !queued_ran.load(Ordering::SeqCst),
        "не начатая работа выполнилась после остановки"
    );
}

#[test]
fn a_result_whose_ticket_is_alive_is_delivered_not_disposed() {
    let workshop = StillWorkshop::start().unwrap();
    let disposed = Arc::new(AtomicBool::new(false));
    let worker_disposed = Arc::clone(&disposed);
    let ticket = workshop
        .submit_or_dispose("тест", || 5_u32, move |_| worker_disposed.store(true, Ordering::SeqCst))
        .unwrap();
    assert!(matches!(poll_until_done(&ticket), TicketPoll::Ready(5)));
    assert!(
        !disposed.load(Ordering::SeqCst),
        "живой билет получил результат, dispose не звался"
    );
}

#[test]
fn stopped_workshop_refuses_new_work_explicitly() {
    let mut workshop = StillWorkshop::start().unwrap();
    // Остановка без уничтожения структуры: так её видит владелец посреди своего `Drop`.
    workshop.jobs = None;
    assert_eq!(workshop.submit("тест", || 1_u32).unwrap_err(), WorkshopError::Stopped);
}
