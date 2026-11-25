//! Память белой: файл в папке приложения за границей `camera_core::white::WhiteMemory` (план white-balance, WB26).
//!
//! Раскладка:
//! * `config` — секция `camera.white_memory` (режим, имена файлов);
//! * `file` — формат файла, чтение с карантином испорченного, атомарная запись;
//! * `writer` — поток записи «ящик на одно письмо», счётчики и последняя ошибка;
//! * этот файл — [`open_white_memory`] (вызывается при старте ДО движка) и [`WhiteMemoryHandoff`], через который
//!   открытая память доезжает от корня композиции до сборки движка.
//!
//! Правило отказов: ни испорченный файл, ни отсутствие каталога, ни неподнявшийся поток не валят запуск.
//! Белая берёт запасной свет (или работает без сохранения), причина — в журнале `fasticamera::white` и в
//! [`WhiteMemoryLoad::Unreadable`], которое движок показывает в статусе.

mod config;
mod file;
mod writer;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use camera_core::white::{NoWhiteMemory, WhiteMemory, WhiteMemoryLoad};

use crate::app_directory::AppDirectory;

pub use config::{WhiteMemoryConfig, WhiteMemoryConfigError, WhiteMemoryMode};
pub use file::{QUARANTINE_FILE_MARKER, WhiteMemoryError, WhiteMemoryFile, WhiteRecordSink, WriteCommit};
pub use writer::{WhiteMemoryDiagnostics, WhiteMemoryStats, WhiteMemoryWriter};

const LOG_TARGET: &str = file::LOG_TARGET;

/// То, что движок получает от памяти: что помнилось к старту и куда писать новое.
pub struct WhiteMemoryParts {
    pub load: WhiteMemoryLoad,
    pub memory: Box<dyn WhiteMemory>,
}

impl Default for WhiteMemoryParts {
    /// Памяти нет (тесты, стенды, режим `none`): ничего не помнится, запись уходит в пустоту.
    fn default() -> Self {
        Self {
            load: WhiteMemoryLoad::NothingRemembered,
            memory: Box::new(NoWhiteMemory),
        }
    }
}

/// Результат открытия памяти.
pub struct OpenedWhiteMemory {
    pub parts: WhiteMemoryParts,
    /// Счётчики записи и последняя ошибка (общие с потоком записи).
    pub diagnostics: WhiteMemoryDiagnostics,
}

/// Открывает память белой по конфигу. Не падает: любой отказ — запись в журнал и работа без сохранения.
pub fn open_white_memory(directory: &AppDirectory, config: &WhiteMemoryConfig) -> OpenedWhiteMemory {
    let diagnostics = WhiteMemoryDiagnostics::default();
    if config.mode == WhiteMemoryMode::None {
        tracing::info!(target: LOG_TARGET, "память белой выключена конфигом (camera.white_memory.mode = none)");
        return OpenedWhiteMemory {
            parts: WhiteMemoryParts::default(),
            diagnostics,
        };
    }
    let opened = WhiteMemoryFile::open(directory.path(), config);
    let load = opened.load;
    let memory: Box<dyn WhiteMemory> = match WhiteMemoryWriter::spawn(
        opened.file,
        diagnostics.clone(),
        config.min_write_interval(),
    ) {
        Ok(writer) => Box::new(writer),
        Err(error) => {
            // Поток не поднялся: белая живёт до конца процесса, но не переживёт перезапуск. Это видно в журнале.
            tracing::error!(target: LOG_TARGET, %error, "поток записи памяти белой не поднялся: белая работает без сохранения");
            Box::new(NoWhiteMemory)
        }
    };
    OpenedWhiteMemory {
        parts: WhiteMemoryParts { load, memory },
        diagnostics,
    }
}

/// Передача открытой памяти от корня композиции к сборке движка. Клоны делят одну ячейку; память забирается
/// ОДИН раз ([`Self::take`]): у движка один владелец записи. Пустая ячейка отдаёт [`WhiteMemoryParts::default`].
#[derive(Clone, Default)]
pub struct WhiteMemoryHandoff {
    slot: Arc<Mutex<Option<WhiteMemoryParts>>>,
    diagnostics: WhiteMemoryDiagnostics,
}

impl WhiteMemoryHandoff {
    /// Ячейка с уже открытой памятью.
    pub fn holding(opened: OpenedWhiteMemory) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(opened.parts))),
            diagnostics: opened.diagnostics,
        }
    }

    /// Забирает память для движка; повторный вызов (и пустая ячейка) — память по умолчанию, то есть без сохранения.
    pub fn take(&self) -> WhiteMemoryParts {
        self.slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_default()
    }

    /// Счётчики записи (пустые, если память не открывалась).
    pub fn diagnostics(&self) -> &WhiteMemoryDiagnostics {
        &self.diagnostics
    }
}

impl fmt::Debug for WhiteMemoryHandoff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("WhiteMemoryHandoff").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod throttle_tests;
