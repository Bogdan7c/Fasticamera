//! Черновик файла снимка в хранилище: «ровно один итог» (опубликован или удалён).
//!
//! Нейтральная часть (тестируется на ПК). Что именно делает платформа (MediaStore на телефоне)
//! скрыто за [`PendingMediaStorage`]; здесь — только инвариант владельца:
//! * публикация или удаление происходят ровно один раз;
//! * черновик, выброшенный без итога (ранний возврат, ошибка, паника), удаляется в `Drop`
//!   с записью в лог и счётчик — недописанный файл в хранилище не остаётся;
//! * записанный файл закрывается ДО публикации (иначе галерея увидит недописанное).

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use shell_contract::still_media::{MediaStoreError, PendingMedia, PublishedMedia};

/// Операции платформы над уже созданным черновиком (по его адресу).
pub trait PendingMediaStorage: Send + Sync {
    /// Сделать файл видимым в галерее (`IS_PENDING = 0`).
    fn publish_pending(&self, uri: &str) -> Result<(), MediaStoreError>;

    /// Удалить черновик из хранилища.
    fn discard_pending(&self, uri: &str) -> Result<(), MediaStoreError>;
}

/// Счётчики итогов черновиков (монотонные с начала процесса).
#[derive(Debug, Default)]
pub struct DraftLedger {
    published: AtomicU64,
    discarded: AtomicU64,
    dropped_without_outcome: AtomicU64,
    publish_failures: AtomicU64,
    discard_failures: AtomicU64,
}

/// Снимок [`DraftLedger`] для диагностики и тестов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DraftLedgerSnapshot {
    /// Опубликовано успешно.
    pub published: u64,
    /// Удалено по явному `discard`.
    pub discarded: u64,
    /// Выброшено без итога и удалено в `Drop` (каждое — признак ошибки в вызывающем коде).
    pub dropped_without_outcome: u64,
    /// Публикация не удалась (черновик после этого удаляется).
    pub publish_failures: u64,
    /// Удаление не удалось: файл мог остаться в хранилище в состоянии «pending».
    pub discard_failures: u64,
}

impl DraftLedger {
    pub fn snapshot(&self) -> DraftLedgerSnapshot {
        DraftLedgerSnapshot {
            published: self.published.load(Ordering::Relaxed),
            discarded: self.discarded.load(Ordering::Relaxed),
            dropped_without_outcome: self.dropped_without_outcome.load(Ordering::Relaxed),
            publish_failures: self.publish_failures.load(Ordering::Relaxed),
            discard_failures: self.discard_failures.load(Ordering::Relaxed),
        }
    }
}

impl DraftLedger {
    /// Убирает запись хранилища, для которой не удалось открыть файл на запись (черновик не создан, `PendingDraft`
    /// нет). Сбой удаления не молчит: лог и `discard_failures` — запись могла остаться в состоянии «pending».
    /// Возвращает результат удаления; первопричина (почему не открылся файл) остаётся за вызывающим.
    pub fn discard_entry_without_draft(
        &self,
        storage: &dyn PendingMediaStorage,
        uri: &str,
    ) -> Result<(), MediaStoreError> {
        let result = storage.discard_pending(uri);
        if let Err(error) = &result {
            self.discard_failures.fetch_add(1, Ordering::Relaxed);
            tracing::error!(target: "fasticamera::still", %uri, %error, "запись без открытого файла не удалена из хранилища: она могла остаться в pending");
        }
        result
    }
}

/// Черновик с открытым для записи файлом.
pub struct PendingDraft {
    uri: String,
    /// `None` — файл уже закрыт (итог принят или закрытие началось).
    sink: Option<Box<dyn Write + Send>>,
    storage: Arc<dyn PendingMediaStorage>,
    ledger: Arc<DraftLedger>,
    /// `true`, пока итог не принят: только тогда `Drop` обязан удалить файл.
    outcome_pending: bool,
}

impl PendingDraft {
    /// `sink` — открытый на запись файл черновика (на телефоне — `File` из `detachFd`).
    pub fn new(
        uri: String,
        sink: Box<dyn Write + Send>,
        storage: Arc<dyn PendingMediaStorage>,
        ledger: Arc<DraftLedger>,
    ) -> Self {
        Self {
            uri,
            sink: Some(sink),
            storage,
            ledger,
            outcome_pending: true,
        }
    }

    /// Закрывает файл (сброс буфера и закрытие дескриптора). Ошибка сброса — наружу.
    fn close_sink(&mut self) -> Result<(), MediaStoreError> {
        match self.sink.take() {
            Some(mut sink) => sink.flush().map_err(|error| MediaStoreError::Io(error.to_string())),
            None => Ok(()),
        }
    }

    /// Удаляет черновик из хранилища; сбой удаления — лог и счётчик, не паника.
    fn remove_from_storage(&mut self) -> Result<(), MediaStoreError> {
        // Сначала закрываем файл: удалять открытый файл незачем держать дескриптор.
        if let Err(error) = self.close_sink() {
            tracing::warn!(target: "fasticamera::still", uri = %self.uri, %error, "черновик: сброс файла перед удалением не удался");
        }
        let result = self.storage.discard_pending(&self.uri);
        if let Err(error) = &result {
            self.ledger.discard_failures.fetch_add(1, Ordering::Relaxed);
            tracing::error!(target: "fasticamera::still", uri = %self.uri, %error, "черновик не удалён из хранилища: файл мог остаться в pending");
        }
        result
    }
}

impl PendingMedia for PendingDraft {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), MediaStoreError> {
        match self.sink.as_mut() {
            Some(sink) => sink
                .write_all(bytes)
                .map_err(|error| MediaStoreError::Io(error.to_string())),
            // Инвариант: sink снимается только вместе с принятием итога, а после него вызвать метод нельзя
            // (`publish`/`discard` забирают `Box<Self>`); ветка защитная и сообщает об ошибке, не паникует.
            None => Err(MediaStoreError::Io("файл черновика уже закрыт".to_owned())),
        }
    }

    fn publish(mut self: Box<Self>) -> Result<PublishedMedia, MediaStoreError> {
        // Итог принят с этого момента: Drop второй раз ничего не удаляет.
        self.outcome_pending = false;
        let published = self.close_sink().and_then(|()| self.storage.publish_pending(&self.uri));
        match published {
            Ok(()) => {
                self.ledger.published.fetch_add(1, Ordering::Relaxed);
                Ok(PublishedMedia { uri: self.uri.clone() })
            }
            Err(error) => {
                // Недопубликованный файл остался бы «pending» до истечения срока — удаляем сами.
                self.ledger.publish_failures.fetch_add(1, Ordering::Relaxed);
                tracing::error!(target: "fasticamera::still", uri = %self.uri, %error, "публикация не удалась, черновик удаляется");
                // Результат удаления учтён счётчиком и логом; вызывающему важна ошибка публикации.
                let _ = self.remove_from_storage();
                Err(error)
            }
        }
    }

    fn discard(mut self: Box<Self>) {
        self.outcome_pending = false;
        // Ошибка удаления уже записана в лог и счётчик; интерфейс `discard` результата не возвращает.
        if self.remove_from_storage().is_ok() {
            self.ledger.discarded.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for PendingDraft {
    fn drop(&mut self) {
        if !self.outcome_pending {
            return;
        }
        self.ledger.dropped_without_outcome.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(target: "fasticamera::still", uri = %self.uri, "черновик выброшен без итога: удаляется");
        // Результат учтён внутри (счётчик и лог): в Drop вернуть его некому.
        let _ = self.remove_from_storage();
    }
}

#[cfg(test)]
#[path = "still_pending_draft_tests.rs"]
mod tests;
