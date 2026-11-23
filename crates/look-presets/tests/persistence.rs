//! Тесты фонового потока записи: склейка, исходы, закрытие.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use common::{TempDir, environment, name, new_library};
use look_contract::{PresetError, StorageFailure};
use look_presets::{
    LibrarySnapshot, LoadOutcome, PRODUCT_MIGRATIONS, PersistenceStats, PresetFileStore, PresetPersistence,
    PresetStoreConfig, SaveCommit, SaveOutcome, SaveRequestOutcome, SnapshotSink,
};

/// Снимок, отличимый по `next_id`.
fn snapshot_with_id(next_id: u64) -> LibrarySnapshot {
    LibrarySnapshot {
        next_id,
        ..new_library().snapshot()
    }
}

/// Приёмник, который блокируется внутри первой записи, пока тест не откроет «ворота».
struct GatedSink {
    written: Arc<Mutex<Vec<u64>>>,
    entered: Sender<()>,
    gate: Receiver<()>,
}

#[allow(clippy::unwrap_used)] // тестовая заглушка: сбой канала = сбой теста
impl SnapshotSink for GatedSink {
    fn write_snapshot(&mut self, snapshot: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        self.entered.send(()).unwrap();
        self.gate.recv().unwrap();
        self.written.lock().unwrap().push(snapshot.next_id);
        Ok(SaveCommit::Durable)
    }
}

struct FailingSink;

impl SnapshotSink for FailingSink {
    fn write_snapshot(&mut self, _: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        Err(PresetError::Storage(StorageFailure::WriteFailed("диск полон".into())))
    }
}

#[test]
fn frequent_requests_are_coalesced_and_last_snapshot_wins() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel();
    let sink = GatedSink {
        written: Arc::clone(&written),
        entered: entered_tx,
        gate: gate_rx,
    };
    let mut persistence = PresetPersistence::spawn(sink, 8).unwrap();

    assert_eq!(
        persistence.request_save(snapshot_with_id(10)),
        SaveRequestOutcome::Queued
    );
    entered_rx.recv().unwrap(); // поток записи взял №10 и застрял в записи

    // Пока он занят, остальные склеиваются: остаётся только последний.
    assert_eq!(
        persistence.request_save(snapshot_with_id(11)),
        SaveRequestOutcome::Queued
    );
    assert_eq!(
        persistence.request_save(snapshot_with_id(12)),
        SaveRequestOutcome::ReplacedPending
    );
    assert_eq!(
        persistence.request_save(snapshot_with_id(13)),
        SaveRequestOutcome::ReplacedPending
    );
    gate_tx.send(()).unwrap(); // дописать №10
    entered_rx.recv().unwrap(); // начал №13
    gate_tx.send(()).unwrap();

    let stats = persistence.close();
    assert_eq!(*written.lock().unwrap(), vec![10, 13], "№11 и №12 склеены");
    assert_eq!(
        stats,
        PersistenceStats {
            saved: 2,
            failed: 0,
            coalesced: 2,
            ..PersistenceStats::default()
        }
    );
    assert_eq!(
        persistence.drain_outcomes(),
        vec![SaveOutcome::Saved, SaveOutcome::Saved]
    );
}

#[test]
fn failed_write_is_reported_through_outcomes_and_counters() {
    let mut persistence = PresetPersistence::spawn(FailingSink, 8).unwrap();
    persistence.request_save(snapshot_with_id(2));
    let stats = persistence.close();
    assert_eq!(
        stats,
        PersistenceStats {
            failed: 1,
            ..PersistenceStats::default()
        }
    );
    assert_eq!(
        persistence.drain_outcomes(),
        vec![SaveOutcome::Failed(PresetError::Storage(StorageFailure::WriteFailed(
            "диск полон".into()
        )))]
    );
    assert!(persistence.drain_outcomes().is_empty(), "исходы забираются один раз");
}

#[test]
fn outcome_queue_drops_oldest_but_counters_stay_exact() {
    let mut persistence = PresetPersistence::spawn(FailingSink, 2).unwrap();
    for id in 2..=4 {
        persistence.request_save(snapshot_with_id(id));
        // Ждём обработки каждой записи: иначе запросы склеились бы.
        while persistence.stats().failed < id - 1 {
            std::thread::yield_now();
        }
    }
    assert_eq!(persistence.close().failed, 3);
    assert_eq!(persistence.drain_outcomes().len(), 2);
}

#[test]
fn requests_after_close_are_refused_and_not_written() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let (entered_tx, _entered_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel();
    gate_tx.send(()).unwrap(); // ворота открыты заранее
    let sink = GatedSink {
        written: Arc::clone(&written),
        entered: entered_tx,
        gate: gate_rx,
    };
    let mut persistence = PresetPersistence::spawn(sink, 4).unwrap();
    persistence.close();
    assert_eq!(
        persistence.request_save(snapshot_with_id(5)),
        SaveRequestOutcome::Closed
    );
    assert_eq!(
        persistence.close(),
        PersistenceStats::default(),
        "повторный close безопасен"
    );
    assert!(written.lock().unwrap().is_empty());
}

#[test]
fn real_file_receives_latest_snapshot_after_close() {
    let dir = TempDir::new("persist");
    let mut opened = PresetFileStore::open(
        dir.path(),
        &PresetStoreConfig::default(),
        &environment(),
        PRODUCT_MIGRATIONS,
    )
    .unwrap();
    let mut persistence = PresetPersistence::spawn(opened.store, 8).unwrap();
    for label in ["One", "Two", "Three", "Four"] {
        opened.library.save_as_new_preset(name(label)).unwrap();
        persistence.request_save(opened.library.snapshot());
    }
    let stats = persistence.close();
    assert_eq!(stats.failed, 0);
    assert_eq!(
        stats.saved + stats.coalesced,
        4,
        "каждый запрос либо записан, либо склеен"
    );
    assert!(stats.saved >= 1);

    let reopened = PresetFileStore::open(
        dir.path(),
        &PresetStoreConfig::default(),
        &environment(),
        PRODUCT_MIGRATIONS,
    )
    .unwrap();
    assert_eq!(reopened.outcome, LoadOutcome::Loaded { migrated_from: None });
    assert_eq!(
        reopened.library.snapshot(),
        opened.library.snapshot(),
        "на диске последний снимок"
    );
}

#[test]
fn drop_flushes_pending_snapshot() {
    let dir = TempDir::new("drop");
    let mut opened = PresetFileStore::open(
        dir.path(),
        &PresetStoreConfig::default(),
        &environment(),
        PRODUCT_MIGRATIONS,
    )
    .unwrap();
    {
        let persistence = PresetPersistence::spawn(opened.store, 8).unwrap();
        opened.library.save_as_new_preset(name("Persisted by drop")).unwrap();
        persistence.request_save(opened.library.snapshot());
    } // Drop дописывает запрос
    let reopened = PresetFileStore::open(
        dir.path(),
        &PresetStoreConfig::default(),
        &environment(),
        PRODUCT_MIGRATIONS,
    )
    .unwrap();
    assert_eq!(reopened.library.presets()[2].name.as_str(), "Persisted by drop");
}

#[test]
fn saving_to_missing_directory_surfaces_storage_error_via_outcomes() {
    let dir = TempDir::new("persistfail");
    let gone = dir.path().join("gone");
    let opened =
        PresetFileStore::open(&gone, &PresetStoreConfig::default(), &environment(), PRODUCT_MIGRATIONS).unwrap();
    let mut persistence = PresetPersistence::spawn(opened.store, 8).unwrap();
    persistence.request_save(opened.library.snapshot());
    persistence.close();
    let outcomes = persistence.drain_outcomes();
    assert!(
        matches!(&outcomes[..], [SaveOutcome::Failed(PresetError::Storage(_))]),
        "{outcomes:?}"
    );
}

/// Приёмник, который паникует при первой же записи (имитация сбоя в приёмнике).
struct PanickingSink;

#[allow(clippy::panic)] // паника — предмет теста
impl SnapshotSink for PanickingSink {
    fn write_snapshot(&mut self, _: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        panic!("тестовая паника приёмника");
    }
}

/// Ждёт, пока поток записи умрёт (страж выхода поднимает флаг в статистике).
fn wait_until_writer_died(persistence: &PresetPersistence) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !persistence.stats().writer_died {
        assert!(std::time::Instant::now() < deadline, "поток записи не объявил о смерти");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn dead_writer_is_visible_to_request_save_and_in_stats() {
    let mut persistence = PresetPersistence::spawn(PanickingSink, 4).unwrap();
    // Первый запрос честно принят: поток ещё жив; он же его и убьёт.
    assert_eq!(
        persistence.request_save(snapshot_with_id(2)),
        SaveRequestOutcome::Queued
    );
    wait_until_writer_died(&persistence);

    // Дальше запросы НЕ изображают постановку в очередь, а честно говорят «поток умер».
    assert_eq!(
        persistence.request_save(snapshot_with_id(3)),
        SaveRequestOutcome::WriterDied
    );
    assert_eq!(
        persistence.request_save(snapshot_with_id(4)),
        SaveRequestOutcome::WriterDied
    );
    let stats = persistence.close();
    assert_eq!(
        stats,
        PersistenceStats {
            rejected_writer_died: 2,
            writer_died: true,
            ..PersistenceStats::default()
        }
    );
    assert!(persistence.drain_outcomes().is_empty(), "записей не было: исходов нет");
}

/// Приёмник, который сразу объявляет отказ писать (как хранилище после неудачного карантина).
struct RefusingSink {
    written: Arc<Mutex<Vec<u64>>>,
}

impl SnapshotSink for RefusingSink {
    fn write_snapshot(&mut self, snapshot: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        self.written.lock().unwrap().push(snapshot.next_id);
        Ok(SaveCommit::Durable)
    }

    fn save_refusal(&self) -> Option<StorageFailure> {
        Some(StorageFailure::SavesRefused("файл не отложен".into()))
    }
}

#[test]
fn refusing_sink_gets_a_distinct_request_outcome_and_is_never_written() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let mut persistence = PresetPersistence::spawn(
        RefusingSink {
            written: Arc::clone(&written),
        },
        4,
    )
    .unwrap();
    assert_eq!(
        persistence.refusal(),
        Some(&StorageFailure::SavesRefused("файл не отложен".into()))
    );
    assert_eq!(
        persistence.request_save(snapshot_with_id(2)),
        SaveRequestOutcome::Refused
    );
    let stats = persistence.close();
    assert_eq!(
        stats,
        PersistenceStats {
            rejected_refused: 1,
            ..PersistenceStats::default()
        }
    );
    assert!(
        written.lock().unwrap().is_empty(),
        "отказанный снимок приёмнику не отдан"
    );
}

/// Приёмник: файл заменён, каталог не подтверждён.
struct UnconfirmedSink;

impl SnapshotSink for UnconfirmedSink {
    fn write_snapshot(&mut self, _: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        Ok(SaveCommit::ReplacedDirectoryUnconfirmed(
            StorageFailure::SyncDirectoryFailed("нет прав".into()),
        ))
    }
}

#[test]
fn unconfirmed_directory_is_its_own_outcome_not_failure_and_not_plain_saved() {
    let mut persistence = PresetPersistence::spawn(UnconfirmedSink, 4).unwrap();
    persistence.request_save(snapshot_with_id(2));
    let stats = persistence.close();
    assert_eq!(
        stats,
        PersistenceStats {
            saved_directory_unconfirmed: 1,
            ..PersistenceStats::default()
        }
    );
    assert_eq!(
        persistence.drain_outcomes(),
        vec![SaveOutcome::SavedDirectoryUnconfirmed(
            StorageFailure::SyncDirectoryFailed("нет прав".into())
        )]
    );
}

#[test]
fn real_store_after_failed_quarantine_refuses_requests_up_front() {
    // Испорченный файл, который не удаётся убрать в карантин (каталог без права записи), — отказ записи.
    // Конструируем проще: на месте основного файла каталог -> read_snapshot даёт Unreadable.
    let dir = TempDir::new("refuse");
    std::fs::create_dir(dir.path().join("look-presets.json")).unwrap();
    let opened = PresetFileStore::open(
        dir.path(),
        &PresetStoreConfig::default(),
        &environment(),
        PRODUCT_MIGRATIONS,
    )
    .unwrap();
    assert!(
        matches!(opened.outcome, LoadOutcome::Unreadable { .. }),
        "{:?}",
        opened.outcome
    );
    let mut persistence = PresetPersistence::spawn(opened.store, 4).unwrap();
    assert_eq!(
        persistence.request_save(opened.library.snapshot()),
        SaveRequestOutcome::Refused
    );
    assert_eq!(persistence.close().rejected_refused, 1);
    assert!(
        std::fs::metadata(dir.path().join("look-presets.json"))
            .unwrap()
            .is_dir(),
        "то, что не смогли прочесть, не тронуто"
    );
}
