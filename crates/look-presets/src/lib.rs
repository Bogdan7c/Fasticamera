//! Хранилище пресетов вида Fasticamera (решения 98, 99, 178, 179).
//!
//! Владелец пресетов и файла. Слои, снизу вверх:
//! - [`config`] — все числа и имена (никаких зашитых значений);
//! - [`library`] — чистая модель в памяти без ввода-вывода: встроенные + свои пресеты,
//!   активный, правки поверх;
//! - [`file`] + [`migration`] — файл со схемой: атомарная запись, миграции, карантин испорченного;
//! - [`persistence`] — фоновый поток записи, рендер диск не ждёт.
//!
//! Видит этот crate только `fasticamera-app` (правило в `scripts/crate-boundaries.toml`).
//! Экран шлёт `LookIntent`, исполнитель в приложении зовёт [`PresetLibrary::apply`] и по
//! [`LibraryChange`] решает: слать ли вид рендеру и просить ли запись файла.

pub mod config;
pub mod file;
pub mod library;
pub mod migration;
pub mod persistence;
mod snapshot;

pub use config::{AsEyeValues, BuiltinCatalog, BuiltinNames, CatalogEnvironment, ConfigError, PresetStoreConfig};
pub use file::{
    CorruptionReason, LoadOutcome, OpenedPresets, PRE_MIGRATION_BACKUP_MARKER, PresetFileStore, QUARANTINE_FILE_MARKER,
    SaveCommit,
};
pub use library::{LibraryChange, PresetLibrary};
pub use migration::{CURRENT_SCHEMA_VERSION, Migration, MigrationError, PRODUCT_MIGRATIONS, migrate_to_current};
pub use persistence::{PersistenceStats, PresetPersistence, SaveOutcome, SaveRequestOutcome, SnapshotSink};
pub use snapshot::{LibrarySnapshot, SnapshotError, UserPreset};
