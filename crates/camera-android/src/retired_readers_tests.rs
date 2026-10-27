use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use super::*;

/// Поддельный reader: число кадров на руках задаёт тест, факт удаления виден снаружи.
struct FakeReader {
    outstanding: Arc<AtomicU64>,
    deleted: Arc<AtomicBool>,
}

struct FakeReaderHandle {
    outstanding: Arc<AtomicU64>,
    deleted: Arc<AtomicBool>,
}

impl FakeReaderHandle {
    fn is_deleted(&self) -> bool {
        self.deleted.load(Ordering::SeqCst)
    }

    /// Потребитель вернул один кадр.
    fn return_one_frame(&self) {
        self.outstanding.fetch_sub(1, Ordering::SeqCst);
    }
}

fn fake_reader(outstanding: u64) -> (FakeReader, FakeReaderHandle) {
    let outstanding = Arc::new(AtomicU64::new(outstanding));
    let deleted = Arc::new(AtomicBool::new(false));
    (
        FakeReader {
            outstanding: Arc::clone(&outstanding),
            deleted: Arc::clone(&deleted),
        },
        FakeReaderHandle { outstanding, deleted },
    )
}

impl RetirableReader for FakeReader {
    fn outstanding_frames(&self) -> u64 {
        self.outstanding.load(Ordering::SeqCst)
    }

    fn delete(self) {
        assert_eq!(self.outstanding_frames(), 0, "удалять reader с кадрами на руках нельзя");
        self.deleted.store(true, Ordering::SeqCst);
    }
}

fn yard() -> (RetiredReaders<FakeReader>, Arc<BackendDiagnostics>) {
    let diagnostics = Arc::new(BackendDiagnostics::new(4));
    (RetiredReaders::new(Arc::clone(&diagnostics)), diagnostics)
}

const SHORT: Duration = Duration::from_millis(1);

#[test]
fn reader_with_frames_on_hand_is_kept_and_not_deleted() {
    let (readers, diagnostics) = yard();
    let (reader, handle) = fake_reader(2);
    readers.retire(reader);
    assert!(!handle.is_deleted());
    assert_eq!(readers.waiting_count(), 1);
    let snapshot = diagnostics.snapshot().reader;
    assert_eq!((snapshot.readers_retired, snapshot.readers_deleted), (1, 0));
}

#[test]
fn reader_is_deleted_on_the_first_reap_after_the_last_frame_came_back() {
    let (readers, diagnostics) = yard();
    let (reader, handle) = fake_reader(2);
    readers.retire(reader);
    handle.return_one_frame();
    assert_eq!(
        readers.reap(),
        ReapReport {
            deleted: 0,
            still_waiting: 1
        }
    );
    assert!(!handle.is_deleted(), "один кадр ещё на руках");
    handle.return_one_frame();
    assert_eq!(
        readers.reap(),
        ReapReport {
            deleted: 1,
            still_waiting: 0
        }
    );
    assert!(handle.is_deleted());
    assert_eq!(diagnostics.snapshot().reader.readers_deleted, 1);
}

#[test]
fn reader_without_frames_on_hand_is_deleted_immediately_on_retire() {
    let (readers, diagnostics) = yard();
    let (reader, handle) = fake_reader(0);
    readers.retire(reader);
    assert!(handle.is_deleted());
    assert_eq!(readers.waiting_count(), 0);
    let snapshot = diagnostics.snapshot().reader;
    assert_eq!((snapshot.readers_retired, snapshot.readers_deleted), (1, 1));
}

#[test]
fn reap_deletes_only_the_ready_reader_and_leaves_the_busy_one_untouched() {
    let (readers, _) = yard();
    let (busy, busy_handle) = fake_reader(1);
    let (done, done_handle) = fake_reader(1);
    readers.retire(busy);
    readers.retire(done);
    done_handle.return_one_frame();
    assert_eq!(
        readers.reap(),
        ReapReport {
            deleted: 1,
            still_waiting: 1
        }
    );
    assert!(done_handle.is_deleted());
    assert!(!busy_handle.is_deleted());
}

#[test]
fn shutdown_leaks_a_reader_whose_frames_never_came_back_and_counts_it() {
    let (readers, diagnostics) = yard();
    let (reader, handle) = fake_reader(1);
    readers.retire(reader);
    let started = Instant::now();
    let report = readers.shutdown(Duration::from_millis(30), SHORT);
    assert_eq!(report, ShutdownReport { deleted: 0, leaked: 1 });
    assert!(
        started.elapsed() >= Duration::from_millis(30),
        "ожидание дошло до срока"
    );
    assert!(
        !handle.is_deleted(),
        "reader с кадрами на руках не удаляется даже при остановке"
    );
    assert_eq!(diagnostics.snapshot().reader.readers_leaked, 1);
    assert_eq!(readers.waiting_count(), 0, "утёкший reader больше не числится в списке");
}

#[test]
fn shutdown_returns_early_when_frames_come_back_from_another_thread() {
    let (readers, diagnostics) = yard();
    let (reader, handle) = fake_reader(1);
    readers.retire(reader);
    let returner = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        handle.return_one_frame();
        handle
    });
    let started = Instant::now();
    let report = readers.shutdown(Duration::from_secs(5), SHORT);
    let handle = returner.join().expect("поток возврата кадра");
    assert_eq!(report, ShutdownReport { deleted: 1, leaked: 0 });
    assert!(handle.is_deleted());
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "ждали возврата, а не весь срок"
    );
    assert_eq!(diagnostics.snapshot().reader.readers_leaked, 0);
}

#[test]
fn shutdown_of_an_empty_list_does_not_wait() {
    let (readers, _) = yard();
    let started = Instant::now();
    assert_eq!(
        readers.shutdown(Duration::from_secs(5), SHORT),
        ShutdownReport { deleted: 0, leaked: 0 }
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}
