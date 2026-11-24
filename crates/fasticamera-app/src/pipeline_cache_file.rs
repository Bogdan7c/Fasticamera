//! Кэш пайплайнов в файле папки приложения ([`PipelineCacheFile`]).
//!
//! Реализует [`PipelineCacheStore`] из рендера: куда класть и откуда брать байты `VkPipelineCache` между запусками.
//! Запись атомарная: данные пишутся во временный файл рядом и переименовываются поверх основного, поэтому убитый
//! посреди записи процесс оставляет прежний целый файл, а не обрезок; перед подменой данные сбрасываются на диск
//! (`sync_all`). Содержимое здесь не разбирается: формат файла, его проверку и предел размера держит
//! `gpu_vulkan::pipeline_cache`, а ошибки чтения и записи здесь типизированы ([`PipelineCacheStoreError`]) и не мешают
//! запуску.

use std::io::Write;
use std::path::PathBuf;

use render_loop::pipeline_cache_store::{PipelineCacheStore, PipelineCacheStoreError};

use crate::app_directory::AppDirectory;

/// Имя файла кэша в папке приложения.
const CACHE_FILE_NAME: &str = "pipeline_cache.bin";
/// Имя временного файла записи (рядом с основным, чтобы переименование не пересекало файловые системы).
const TEMP_FILE_NAME: &str = "pipeline_cache.bin.tmp";

/// Файл кэша пайплайнов в папке приложения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineCacheFile {
    file: PathBuf,
    temp_file: PathBuf,
}

impl PipelineCacheFile {
    /// Файл кэша в проверенной папке приложения.
    pub fn in_app_directory(directory: &AppDirectory) -> Self {
        Self {
            file: directory.path().join(CACHE_FILE_NAME),
            temp_file: directory.path().join(TEMP_FILE_NAME),
        }
    }

    fn read_error(&self, error: &std::io::Error) -> PipelineCacheStoreError {
        PipelineCacheStoreError::Read {
            place: self.file.display().to_string(),
            reason: error.to_string(),
        }
    }

    fn write_error(&self, error: &std::io::Error) -> PipelineCacheStoreError {
        PipelineCacheStoreError::Write {
            place: self.file.display().to_string(),
            reason: error.to_string(),
        }
    }
}

impl PipelineCacheStore for PipelineCacheFile {
    fn load(&self) -> Result<Option<Vec<u8>>, PipelineCacheStoreError> {
        match std::fs::read(&self.file) {
            Ok(data) => Ok(Some(data)),
            // Первого сохранения ещё не было: это штатный случай, а не ошибка.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(self.read_error(&error)),
        }
    }

    fn save(&self, data: &[u8]) -> Result<(), PipelineCacheStoreError> {
        let mut temp = std::fs::File::create(&self.temp_file).map_err(|error| self.write_error(&error))?;
        temp.write_all(data).map_err(|error| self.write_error(&error))?;
        // Данные на диск ДО подмены файла: иначе после внезапного выключения переименование может пережить данные
        // (файл нужной длины, но с нулями внутри). Такой файл поймала бы контрольная сумма, но кэш был бы потерян.
        temp.sync_all().map_err(|error| self.write_error(&error))?;
        drop(temp);
        // Переименование атомарно в пределах одной файловой системы: читатель видит либо старый файл, либо новый.
        std::fs::rename(&self.temp_file, &self.file).map_err(|error| self.write_error(&error))
    }
}

#[cfg(test)]
#[path = "pipeline_cache_file_tests.rs"]
mod tests;
