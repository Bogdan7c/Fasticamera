//! Файл пресетов: загрузка с миграцией и карантином, атомарная запись.
//!
//! Гарантии:
//! - запись — «временный файл рядом → `write_all` → `sync_all` → `rename` поверх → sync каталога»:
//!   прерванная запись оставляет прежний файл целым (виден либо старый, либо новый целиком);
//! - испорченный файл или файл более новой схемы НЕ затирается: переименовывается в
//!   `<имя>.corrupt-<unix-мс>` (решение 179), работают встроенные пресеты;
//! - если убрать файл в сторону не удалось, сохранения ЗАПРЕЩЕНЫ явной ошибкой (не затираем).

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use look_contract::{LookSettings, PresetError, PresetId, StorageFailure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{ConfigError, PresetStoreConfig};
use crate::library::PresetLibrary;
use crate::migration::{CURRENT_SCHEMA_VERSION, Migration, MigrationError, SCHEMA_VERSION_FIELD, migrate_to_current};
use crate::persistence::SnapshotSink;
use crate::snapshot::{LibrarySnapshot, SnapshotError, UserPreset};

/// Метка карантинного файла: `<имя файла>.corrupt-<unix-мс>`. Формат имени — часть
/// контракта диагностики (владелец ищет такие файлы руками), поэтому не конфиг.
pub const QUARANTINE_FILE_MARKER: &str = ".corrupt-";

/// Цель для `tracing`: события хранилища пресетов фильтруются одним именем.
pub(crate) const LOG_TARGET: &str = "fasticamera::look_presets";

/// Почему файл признан непригодным.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CorruptionReason {
    /// Не JSON.
    #[error("файл не JSON: {0}")]
    NotJson(String),
    /// Нет числового поля `schema_version`.
    #[error("нет числового поля schema_version")]
    MissingSchemaVersion,
    /// Файл записан более новой версией приложения.
    #[error("схема файла {found} новее поддерживаемой {supported}")]
    NewerSchema {
        /// Версия в файле.
        found: u32,
        /// Максимальная поддерживаемая.
        supported: u32,
    },
    /// Миграция не удалась или шага нет.
    #[error("миграция схемы не удалась: {0}")]
    MigrationFailed(MigrationError),
    /// Форма JSON не совпала со схемой (нет поля, лишнее поле, ручка вне диапазона, плохое имя).
    #[error("форма файла не совпала со схемой: {0}")]
    InvalidShape(String),
    /// Форма верна, но содержимое нарушает инварианты библиотеки.
    #[error("содержимое файла нарушает инварианты: {0}")]
    InvalidContent(SnapshotError),
}

/// Исход загрузки файла.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    /// Файла нет: чистая библиотека со встроенными.
    Missing,
    /// Файл прочитан; `migrated_from` — исходная версия, если применялись миграции.
    Loaded {
        /// Версия до миграции (`None` — миграция не понадобилась).
        migrated_from: Option<u32>,
    },
    /// Файл был испорчен или новее схемы и убран в сторону; работаем с чистой библиотекой.
    Quarantined {
        /// Что с ним было не так.
        reason: CorruptionReason,
        /// Куда файл перемещён (содержимое не менялось).
        moved_to: PathBuf,
    },
    /// Файл испорчен, но убрать его в сторону не удалось: файл не тронут, сохранения отключены.
    QuarantineFailed {
        /// Что с ним было не так.
        reason: CorruptionReason,
        /// Почему не удалось переименовать.
        failure: StorageFailure,
    },
    /// Файл есть, но прочитать его не удалось: не трогаем, сохранения отключены.
    Unreadable {
        /// Причина.
        failure: StorageFailure,
    },
}

/// Результат открытия хранилища.
#[derive(Debug)]
pub struct OpenedPresets {
    /// Библиотека для работы в памяти.
    pub library: PresetLibrary,
    /// Файл для записи (передаётся фоновому потоку записи).
    pub store: PresetFileStore,
    /// Как прошла загрузка (для диагностики и экрана).
    pub outcome: LoadOutcome,
}

/// Чем закончилась успешная запись: файл заменён в обоих случаях, различие — в подтверждении каталога.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveCommit {
    /// Файл заменён и каталог сброшен на диск.
    Durable,
    /// Файл заменён (новое содержимое уже на месте), но сбросить каталог на диск не удалось.
    /// Это НЕ потеря данных и не ошибка записи: лишь нет гарантии, что переименование переживёт
    /// отключение питания в этот миг.
    ReplacedDirectoryUnconfirmed(StorageFailure),
}

/// Метка резервной копии файла старой схемы: `<имя файла>.pre-v<версия>` (формат — часть контракта
/// диагностики, как и [`QUARANTINE_FILE_MARKER`]).
pub const PRE_MIGRATION_BACKUP_MARKER: &str = ".pre-v";

/// Форма файла на диске (версия 1).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetFileDocument {
    schema_version: u32,
    next_id: u64,
    active: PresetId,
    edits: Option<LookSettings>,
    user_presets: Vec<UserPreset>,
}

impl PresetFileDocument {
    fn from_snapshot(snapshot: &LibrarySnapshot) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            next_id: snapshot.next_id,
            active: snapshot.active,
            edits: snapshot.edits.clone(),
            user_presets: snapshot.user_presets.clone(),
        }
    }

    fn into_snapshot(self) -> LibrarySnapshot {
        LibrarySnapshot {
            next_id: self.next_id,
            active: self.active,
            edits: self.edits,
            user_presets: self.user_presets,
        }
    }
}

/// Файл пресетов: владеет путями и запретом записи после неудачного карантина.
#[derive(Debug)]
pub struct PresetFileStore {
    path: PathBuf,
    temp_path: PathBuf,
    /// `Some` — писать нельзя (причина), чтобы не затереть непрочитанный файл.
    saves_refused: Option<StorageFailure>,
    /// `Some(версия)` — файл прочитан с миграции из этой версии и ещё не сохранён в копию:
    /// первое сохранение обязано сначала скопировать исходный файл (решение 179), иначе оно
    /// затрёт файл старой схемы. `Cell`: `save` берёт `&self`, а флаг сбрасывается один раз.
    pending_backup_of_version: Cell<Option<u32>>,
}

impl PresetFileStore {
    /// Открывает хранилище в папке приложения: проверяет конфиг, читает файл, мигрирует,
    /// при порче убирает в карантин. Встроенные работают при любом исходе.
    pub fn open(
        directory: &Path,
        config: &PresetStoreConfig,
        migrations: &[Migration],
    ) -> Result<OpenedPresets, ConfigError> {
        let catalog = config.builtin_catalog()?;
        let path = directory.join(&config.file_name);
        let temp_path = directory.join(format!("{}{}", config.file_name, config.temp_file_suffix));
        let mut store = Self {
            path,
            temp_path,
            saves_refused: None,
            pending_backup_of_version: Cell::new(None),
        };

        let fresh = || PresetLibrary::new(catalog.clone());
        let (library, outcome) = match store.read_snapshot(migrations) {
            ReadResult::Missing => (fresh(), LoadOutcome::Missing),
            ReadResult::Loaded {
                snapshot,
                migrated_from,
            } => match PresetLibrary::from_snapshot(catalog.clone(), snapshot) {
                Ok(library) => {
                    // Файл старой схемы ещё лежит на диске нетронутым: перед первой записью сохраним копию.
                    store.pending_backup_of_version.set(migrated_from);
                    (library, LoadOutcome::Loaded { migrated_from })
                }
                Err(error) => (fresh(), store.quarantine(CorruptionReason::InvalidContent(error))),
            },
            ReadResult::Corrupt(reason) => (fresh(), store.quarantine(reason)),
            ReadResult::Unreadable(failure) => {
                tracing::warn!(target: LOG_TARGET, %failure, "файл пресетов не прочитан, сохранения отключены");
                store.saves_refused = Some(failure.clone());
                (fresh(), LoadOutcome::Unreadable { failure })
            }
        };
        Ok(OpenedPresets {
            library,
            store,
            outcome,
        })
    }

    /// Путь основного файла.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Атомарно записывает снимок. Ошибка — `PresetError::Storage` с различимой причиной;
    /// прежний файл при ошибке остаётся целым. Успех различает [`SaveCommit`]: каталог подтверждён
    /// на диске или нет (файл при этом уже заменён).
    ///
    /// Первое сохранение после миграции сначала копирует исходный файл в `<имя>.pre-v<версия>`;
    /// не вышло скопировать — запись отказана (`BackupFailed`), старый файл не тронут.
    pub fn save(&self, snapshot: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        if let Some(refusal) = &self.saves_refused {
            return Err(PresetError::Storage(StorageFailure::SavesRefused(refusal.to_string())));
        }
        let document = PresetFileDocument::from_snapshot(snapshot);
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|error| storage(StorageFailure::SerializeFailed(error.to_string())))?;

        self.back_up_pre_migration_file()?;
        self.write_temp_file(&bytes)?;
        if let Err(error) = fs::rename(&self.temp_path, &self.path) {
            self.discard_temp_file();
            return Err(storage(StorageFailure::ReplaceFailed(error.to_string())));
        }
        // Файл уже заменён: сбой сброса каталога — не ошибка записи, а отдельный исход.
        Ok(match self.sync_directory() {
            Ok(()) => SaveCommit::Durable,
            Err(failure) => SaveCommit::ReplacedDirectoryUnconfirmed(failure),
        })
    }

    /// Копирует файл старой схемы рядом (`create_new`: уже существующую копию не затираем, берём
    /// следующий суффикс). Вызывается ровно один раз после миграции; флаг сбрасывается после успеха.
    fn back_up_pre_migration_file(&self) -> Result<(), PresetError> {
        let Some(version) = self.pending_backup_of_version.get() else {
            return Ok(());
        };
        let source = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            // Файл исчез между чтением и записью: копировать нечего, терять тоже нечего.
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.pending_backup_of_version.set(None);
                return Ok(());
            }
            Err(error) => return Err(storage(StorageFailure::BackupFailed(error.to_string()))),
        };
        let base = format!("{}{PRE_MIGRATION_BACKUP_MARKER}{version}", self.file_name_lossy());
        let mut attempt = 0_u32;
        loop {
            let name = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-{attempt}")
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.path.with_file_name(&name))
            {
                Ok(mut copy) => {
                    copy.write_all(&source)
                        .and_then(|()| copy.sync_all())
                        .map_err(|error| storage(StorageFailure::BackupFailed(error.to_string())))?;
                    tracing::info!(target: LOG_TARGET, backup = %name, "файл пресетов старой схемы сохранён перед миграцией");
                    self.pending_backup_of_version.set(None);
                    return Ok(());
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => attempt += 1,
                Err(error) => return Err(storage(StorageFailure::BackupFailed(error.to_string()))),
            }
        }
    }

    /// Пишет и сбрасывает на диск временный файл. Старый временный файл (от прерванной
    /// записи) перезаписывается: основной файл он не затрагивает.
    fn write_temp_file(&self, bytes: &[u8]) -> Result<(), PresetError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.temp_path)
            .map_err(|error| storage(StorageFailure::CreateTempFailed(error.to_string())))?;
        let written = file
            .write_all(bytes)
            .map_err(|error| storage(StorageFailure::WriteFailed(error.to_string())))
            .and_then(|()| {
                file.sync_all()
                    .map_err(|error| storage(StorageFailure::SyncFailed(error.to_string())))
            });
        if written.is_err() {
            self.discard_temp_file();
        }
        written
    }

    /// Убирает временный файл после сбоя; сама неудача уборки не критична, но не молчит.
    fn discard_temp_file(&self) {
        if let Err(error) = fs::remove_file(&self.temp_path)
            && error.kind() != ErrorKind::NotFound
        {
            tracing::warn!(target: LOG_TARGET, %error, "не удалось убрать временный файл пресетов");
        }
    }

    /// Сбрасывает каталог на диск, чтобы переименование пережило отключение питания.
    /// Только unix: на других системах открыть каталог как файл нельзя.
    #[cfg(unix)]
    fn sync_directory(&self) -> Result<(), StorageFailure> {
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        File::open(directory)
            .and_then(|handle| handle.sync_all())
            .map_err(|error| StorageFailure::SyncDirectoryFailed(error.to_string()))
    }

    #[cfg(not(unix))]
    fn sync_directory(&self) -> Result<(), StorageFailure> {
        Ok(())
    }

    /// Читает файл и приводит его к снимку (без проверки инвариантов библиотеки).
    fn read_snapshot(&self, migrations: &[Migration]) -> ReadResult {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return ReadResult::Missing,
            Err(error) => {
                return ReadResult::Unreadable(StorageFailure::ReadFailed(error.to_string()));
            }
        };
        let document: Value = match serde_json::from_slice(&bytes) {
            Ok(document) => document,
            Err(error) => return ReadResult::Corrupt(CorruptionReason::NotJson(error.to_string())),
        };
        let Some(found) = document
            .get(SCHEMA_VERSION_FIELD)
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
        else {
            return ReadResult::Corrupt(CorruptionReason::MissingSchemaVersion);
        };
        if found > CURRENT_SCHEMA_VERSION {
            return ReadResult::Corrupt(CorruptionReason::NewerSchema {
                found,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        let migrated_from = (found < CURRENT_SCHEMA_VERSION).then_some(found);
        let document = match migrate_to_current(document, found, migrations) {
            Ok(document) => document,
            Err(error) => return ReadResult::Corrupt(CorruptionReason::MigrationFailed(error)),
        };
        match serde_json::from_value::<PresetFileDocument>(document) {
            Ok(parsed) => ReadResult::Loaded {
                snapshot: parsed.into_snapshot(),
                migrated_from,
            },
            Err(error) => ReadResult::Corrupt(CorruptionReason::InvalidShape(error.to_string())),
        }
    }

    /// Убирает испорченный файл в `<имя>.corrupt-<unix-мс>`; при неудаче отключает сохранения.
    fn quarantine(&mut self, reason: CorruptionReason) -> LoadOutcome {
        let target = self.free_quarantine_path();
        match fs::rename(&self.path, &target) {
            Ok(()) => {
                tracing::warn!(target: LOG_TARGET, %reason, moved_to = %target.display(),
                    "файл пресетов испорчен и отложен в карантин");
                LoadOutcome::Quarantined {
                    reason,
                    moved_to: target,
                }
            }
            Err(error) => {
                let failure = StorageFailure::ReplaceFailed(error.to_string());
                tracing::warn!(target: LOG_TARGET, %reason, %failure,
                    "файл пресетов испорчен и не отложен: сохранения отключены");
                self.saves_refused = Some(failure.clone());
                LoadOutcome::QuarantineFailed { reason, failure }
            }
        }
    }

    /// Свободное имя карантинного файла: время в мс, при совпадении — числовой хвост.
    fn free_quarantine_path(&self) -> PathBuf {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            // Часы до 1970 года: берём 0, имя всё равно уникализируется хвостом.
            .unwrap_or(0);
        let base = format!("{}{QUARANTINE_FILE_MARKER}{now_ms}", self.file_name_lossy());
        let mut candidate = self.path.with_file_name(&base);
        let mut attempt = 1_u32;
        while candidate.exists() {
            candidate = self.path.with_file_name(format!("{base}-{attempt}"));
            attempt += 1;
        }
        candidate
    }

    fn file_name_lossy(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

impl SnapshotSink for PresetFileStore {
    fn write_snapshot(&mut self, snapshot: &LibrarySnapshot) -> Result<SaveCommit, PresetError> {
        self.save(snapshot)
    }

    fn save_refusal(&self) -> Option<StorageFailure> {
        self.saves_refused.clone()
    }
}

/// Промежуточный результат чтения файла.
enum ReadResult {
    Missing,
    Loaded {
        snapshot: LibrarySnapshot,
        migrated_from: Option<u32>,
    },
    Corrupt(CorruptionReason),
    Unreadable(StorageFailure),
}

fn storage(failure: StorageFailure) -> PresetError {
    PresetError::Storage(failure)
}
