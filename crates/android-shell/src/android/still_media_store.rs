//! Хранилище фото снимка: `MediaStoreService` поверх Kotlin `StillMediaStore` (MediaStore).
//!
//! Rust владеет файловым дескриптором черновика (`detachFd`) и пишет в него байты; Kotlin только
//! создаёт запись, публикует и удаляет. Инвариант «один итог» держит нейтральный
//! [`PendingDraft`](crate::still_pending_draft::PendingDraft).
//! Все методы синхронные, зовутся с рабочего потока записи снимка.

use std::sync::Arc;

use jni::objects::{JObject, JString, JValue};
use jni::{Env, jni_sig, jni_str};
use shell_contract::still_media::{MediaStoreError, MediaStoreService, PendingMedia, PendingMediaRequest};

use super::runtime::ShellRuntime;
use super::services::AndroidShellServices;
use super::still_jni::{call_platform, media_store_object};
use super::still_unsafe::file_from_detached_fd;
use crate::platform_call_error::PlatformCallError;
use crate::still_orphan_sweep::OrphanSweepReport;
use crate::still_pending_draft::{DraftLedger, DraftLedgerSnapshot, PendingDraft, PendingMediaStorage};

/// Хранилище фото на телефоне. Создаётся один раз при сборке приложения (`new`).
pub struct AndroidMediaStoreService {
    storage: Arc<AndroidPendingStorage>,
    ledger: Arc<DraftLedger>,
}

impl AndroidMediaStoreService {
    pub fn new(services: &AndroidShellServices) -> Self {
        Self {
            storage: Arc::new(AndroidPendingStorage {
                runtime: Arc::clone(services.runtime()),
            }),
            ledger: Arc::new(DraftLedger::default()),
        }
    }

    /// Итоги черновиков с начала процесса (диагностика: `dropped_without_outcome` и
    /// `discard_failures` в норме нули).
    pub fn draft_ledger(&self) -> DraftLedgerSnapshot {
        self.ledger.snapshot()
    }

    /// Итог уборки осиротевших черновиков прошлого процесса (её делает Kotlin на своём потоке при старте
    /// платформы). Платформа не подключена — `NotStarted`; сбой самого вызова — `Failed`, не молчание.
    pub fn orphan_sweep_report(&self) -> OrphanSweepReport {
        self.storage.orphan_sweep_report()
    }
}

impl MediaStoreService for AndroidMediaStoreService {
    fn create_pending(&self, request: &PendingMediaRequest) -> Result<Box<dyn PendingMedia>, MediaStoreError> {
        let uri = self.storage.create_entry(request)?;
        // Запись в MediaStore уже есть: любой отказ дальше обязан её убрать, иначе останется «pending».
        let file = match self.storage.open_write_file(&uri) {
            Ok(file) => file,
            Err(error) => {
                // Сбой удаления учтён журналом (лог + `discard_failures`); вызывающему важна первопричина.
                let _ = self.ledger.discard_entry_without_draft(self.storage.as_ref(), &uri);
                return Err(error);
            }
        };
        Ok(Box::new(PendingDraft::new(
            uri,
            Box::new(file),
            Arc::clone(&self.storage) as Arc<dyn PendingMediaStorage>,
            Arc::clone(&self.ledger),
        )))
    }
}

/// Вызовы `StillMediaStore` по адресу черновика.
struct AndroidPendingStorage {
    runtime: Arc<ShellRuntime>,
}

impl AndroidPendingStorage {
    fn create_entry(&self, request: &PendingMediaRequest) -> Result<String, MediaStoreError> {
        let uri = call_platform(&self.runtime, |env, platform| {
            let store = media_store_object(env, platform)?;
            let display_name = env.new_string(&request.display_name)?;
            let mime_type = env.new_string(&request.mime_type)?;
            let relative_path = env.new_string(&request.relative_path)?;
            let returned = env
                .call_method(
                    &store,
                    jni_str!("createPending"),
                    jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)Ljava/lang/String;"),
                    &[
                        JValue::Object(&display_name),
                        JValue::Object(&mime_type),
                        JValue::Object(&relative_path),
                        JValue::Long(request.date_taken_millis),
                    ],
                )?
                .l()?;
            read_string(env, returned)
        })?;
        Ok(uri)
    }

    /// `StillMediaStore.orphanSweepReport()` → состояние уборки.
    fn orphan_sweep_report(&self) -> OrphanSweepReport {
        let text = call_platform(&self.runtime, |env, platform| {
            let store = media_store_object(env, platform)?;
            let returned = env
                .call_method(
                    &store,
                    jni_str!("orphanSweepReport"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            read_string(env, returned)
        });
        match text {
            Ok(text) => OrphanSweepReport::from_platform_text(&text),
            Err(PlatformCallError::NotAttached) => OrphanSweepReport::NotStarted,
            Err(PlatformCallError::Failed(reason)) => OrphanSweepReport::Failed { reason },
        }
    }

    /// Открывает файл черновика на запись и забирает дескриптор во владение Rust.
    fn open_write_file(&self, uri: &str) -> Result<std::fs::File, MediaStoreError> {
        let fd = call_platform(&self.runtime, |env, platform| {
            let store = media_store_object(env, platform)?;
            let uri_text = env.new_string(uri)?;
            env.call_method(
                &store,
                jni_str!("openWriteFd"),
                jni_sig!("(Ljava/lang/String;)I"),
                &[JValue::Object(&uri_text)],
            )?
            .i()
        })?;
        file_from_detached_fd(fd).map_err(MediaStoreError::Platform)
    }

    /// Вызов `StillMediaStore.<method>(uri)` без результата.
    fn call_with_uri(&self, uri: &str, method: UriMethod) -> Result<(), MediaStoreError> {
        call_platform(&self.runtime, |env, platform| {
            let store = media_store_object(env, platform)?;
            let uri_text = env.new_string(uri)?;
            let arguments = [JValue::Object(&uri_text)];
            match method {
                UriMethod::Publish => env.call_method(
                    &store,
                    jni_str!("publish"),
                    jni_sig!("(Ljava/lang/String;)V"),
                    &arguments,
                )?,
                UriMethod::Discard => env.call_method(
                    &store,
                    jni_str!("discard"),
                    jni_sig!("(Ljava/lang/String;)V"),
                    &arguments,
                )?,
            };
            Ok(())
        })?;
        Ok(())
    }
}

/// Какой метод `StillMediaStore` с адресом вызвать (литералы имён JNI должны быть на месте вызова).
#[derive(Clone, Copy)]
enum UriMethod {
    Publish,
    Discard,
}

impl PendingMediaStorage for AndroidPendingStorage {
    fn publish_pending(&self, uri: &str) -> Result<(), MediaStoreError> {
        self.call_with_uri(uri, UriMethod::Publish)
    }

    fn discard_pending(&self, uri: &str) -> Result<(), MediaStoreError> {
        self.call_with_uri(uri, UriMethod::Discard)
    }
}

/// Строка Java → Rust. `null` — нарушение контракта Kotlin-метода (он объявлен с ненулевой строкой).
fn read_string(env: &mut Env<'_>, returned: JObject<'_>) -> jni::errors::Result<String> {
    if returned.is_null() {
        return Err(jni::errors::Error::NullPtr("Kotlin вернул null вместо строки"));
    }
    let text = env.cast_local::<JString>(returned)?;
    text.try_to_string(env)
}
