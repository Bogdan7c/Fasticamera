use std::io;
use std::sync::Mutex;

use super::*;

/// Что платформа видела: порядок вызовов хранилища.
#[derive(Default)]
struct FakeStorage {
    calls: Mutex<Vec<String>>,
    fail_publish: bool,
    fail_discard: bool,
    /// Файл черновика: при публикации запоминаем, был ли он уже закрыт.
    watched_file: Option<Arc<Mutex<FakeFileState>>>,
    file_closed_at_publish: Mutex<Option<bool>>,
}

impl FakeStorage {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl PendingMediaStorage for FakeStorage {
    fn publish_pending(&self, uri: &str) -> Result<(), MediaStoreError> {
        self.calls.lock().unwrap().push(format!("publish {uri}"));
        if let Some(file) = &self.watched_file {
            *self.file_closed_at_publish.lock().unwrap() = Some(file.lock().unwrap().closed);
        }
        if self.fail_publish {
            return Err(MediaStoreError::Platform("publish отказал".to_owned()));
        }
        Ok(())
    }

    fn discard_pending(&self, uri: &str) -> Result<(), MediaStoreError> {
        self.calls.lock().unwrap().push(format!("discard {uri}"));
        if self.fail_discard {
            return Err(MediaStoreError::Platform("delete отказал".to_owned()));
        }
        Ok(())
    }
}

/// Общее состояние файла-заглушки: что записано и закрыт ли он.
#[derive(Default)]
struct FakeFileState {
    bytes: Vec<u8>,
    closed: bool,
}

/// Настройки и состояние файла; сам «дескриптор» — [`FakeSink`], который уходит в черновик.
#[derive(Clone, Default)]
struct FakeFile {
    state: Arc<Mutex<FakeFileState>>,
    fail_flush: bool,
    fail_write: bool,
}

impl FakeFile {
    fn open_sink(&self) -> FakeSink {
        FakeSink {
            state: Arc::clone(&self.state),
            fail_flush: self.fail_flush,
            fail_write: self.fail_write,
        }
    }
}

/// Открытый дескриптор: уничтожение = закрытие файла.
struct FakeSink {
    state: Arc<Mutex<FakeFileState>>,
    fail_flush: bool,
    fail_write: bool,
}

impl Write for FakeSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            return Err(io::Error::other("диск полон"));
        }
        self.state.lock().unwrap().bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.fail_flush {
            return Err(io::Error::other("flush отказал"));
        }
        Ok(())
    }
}

impl Drop for FakeSink {
    fn drop(&mut self) {
        self.state.lock().unwrap().closed = true;
    }
}

struct Rig {
    storage: Arc<FakeStorage>,
    ledger: Arc<DraftLedger>,
    file: FakeFile,
}

fn rig(mut storage: FakeStorage, file: FakeFile) -> (Rig, PendingDraft) {
    storage.watched_file = Some(Arc::clone(&file.state));
    let storage = Arc::new(storage);
    let ledger = Arc::new(DraftLedger::default());
    let draft = PendingDraft::new(
        "content://media/1".to_owned(),
        Box::new(file.open_sink()),
        Arc::clone(&storage) as Arc<dyn PendingMediaStorage>,
        Arc::clone(&ledger),
    );
    (Rig { storage, ledger, file }, draft)
}

#[test]
fn published_draft_has_bytes_closed_file_and_single_publish() {
    let (rig, draft) = rig(FakeStorage::default(), FakeFile::default());
    let mut draft: Box<dyn PendingMedia> = Box::new(draft);
    draft.write_all(b"abc").unwrap();
    draft.write_all(b"def").unwrap();

    let published = draft.publish().unwrap();

    assert_eq!(published.uri, "content://media/1");
    assert_eq!(rig.file.state.lock().unwrap().bytes, b"abcdef");
    assert_eq!(
        *rig.storage.file_closed_at_publish.lock().unwrap(),
        Some(true),
        "файл закрыт ДО публикации"
    );
    // Drop после публикации ничего не удаляет: ровно один итог.
    assert_eq!(rig.storage.calls(), vec!["publish content://media/1"]);
    assert_eq!(
        rig.ledger.snapshot(),
        DraftLedgerSnapshot {
            published: 1,
            ..Default::default()
        }
    );
}

#[test]
fn discarded_draft_is_deleted_once_and_not_published() {
    let (rig, draft) = rig(FakeStorage::default(), FakeFile::default());
    Box::new(draft).discard();

    assert_eq!(rig.storage.calls(), vec!["discard content://media/1"]);
    assert!(rig.file.state.lock().unwrap().closed);
    assert_eq!(
        rig.ledger.snapshot(),
        DraftLedgerSnapshot {
            discarded: 1,
            ..Default::default()
        }
    );
}

#[test]
fn dropped_draft_is_deleted_and_counted_as_dropped() {
    let (rig, mut draft) = rig(FakeStorage::default(), FakeFile::default());
    draft.write_all(b"half-written").unwrap();
    drop(draft);

    assert_eq!(rig.storage.calls(), vec!["discard content://media/1"]);
    assert!(rig.file.state.lock().unwrap().closed);
    assert_eq!(
        rig.ledger.snapshot(),
        DraftLedgerSnapshot {
            dropped_without_outcome: 1,
            ..Default::default()
        }
    );
}

#[test]
fn failed_publish_removes_draft_and_returns_the_publish_error() {
    let storage = FakeStorage {
        fail_publish: true,
        ..Default::default()
    };
    let (rig, draft) = rig(storage, FakeFile::default());

    let error = Box::new(draft).publish().unwrap_err();

    assert_eq!(error, MediaStoreError::Platform("publish отказал".to_owned()));
    // Попытка публикации, затем ровно одно удаление (Drop не удаляет повторно).
    assert_eq!(
        rig.storage.calls(),
        vec!["publish content://media/1", "discard content://media/1"]
    );
    let snapshot = rig.ledger.snapshot();
    assert_eq!((snapshot.published, snapshot.publish_failures), (0, 1));
}

#[test]
fn flush_failure_blocks_publication_and_removes_draft() {
    let file = FakeFile {
        fail_flush: true,
        ..Default::default()
    };
    let (rig, draft) = rig(FakeStorage::default(), file);

    let error = Box::new(draft).publish().unwrap_err();

    assert!(matches!(error, MediaStoreError::Io(text) if text.contains("flush")));
    // Недописанный файл не публикуется ни при каких условиях.
    assert_eq!(rig.storage.calls(), vec!["discard content://media/1"]);
}

#[test]
fn discard_failure_is_counted_and_not_repeated_by_drop() {
    let storage = FakeStorage {
        fail_discard: true,
        ..Default::default()
    };
    let (rig, draft) = rig(storage, FakeFile::default());
    Box::new(draft).discard();

    assert_eq!(rig.storage.calls().len(), 1, "повторного удаления из Drop нет");
    let snapshot = rig.ledger.snapshot();
    assert_eq!((snapshot.discarded, snapshot.discard_failures), (0, 1));
}

#[test]
fn write_error_is_io_and_later_drop_still_cleans_up() {
    let file = FakeFile {
        fail_write: true,
        ..Default::default()
    };
    let (rig, mut draft) = rig(FakeStorage::default(), file);

    let error = draft.write_all(b"x").unwrap_err();
    assert_eq!(error, MediaStoreError::Io("диск полон".to_owned()));
    // Вызывающий вернул ошибку наверх и выбросил черновик — он обязан исчезнуть из хранилища.
    drop(draft);
    assert_eq!(rig.storage.calls(), vec!["discard content://media/1"]);
}

#[test]
fn uri_is_reported_and_untouched_by_writes() {
    let (rig, mut draft) = rig(FakeStorage::default(), FakeFile::default());
    assert_eq!(draft.uri(), "content://media/1");
    draft.write_all(b"abc").unwrap();
    assert_eq!(draft.uri(), "content://media/1");
    // Запись не трогает хранилище: оно вызывается только итогом.
    assert!(rig.storage.calls().is_empty());
    Box::new(draft).discard();
}

#[test]
fn entry_without_draft_is_discarded_and_failure_is_counted() {
    let ledger = DraftLedger::default();
    let storage = FakeStorage::default();
    assert!(ledger.discard_entry_without_draft(&storage, "content://e/1").is_ok());
    assert_eq!(storage.calls(), vec!["discard content://e/1".to_owned()]);
    assert_eq!(ledger.snapshot().discard_failures, 0);

    let failing = FakeStorage {
        fail_discard: true,
        ..FakeStorage::default()
    };
    let result = ledger.discard_entry_without_draft(&failing, "content://e/2");
    assert!(matches!(result, Err(MediaStoreError::Platform(_))));
    // Остальные итоги журнал не трогает: запись без черновика — не публикация и не явный discard.
    assert_eq!(
        ledger.snapshot(),
        DraftLedgerSnapshot {
            discard_failures: 1,
            ..DraftLedgerSnapshot::default()
        }
    );
}
