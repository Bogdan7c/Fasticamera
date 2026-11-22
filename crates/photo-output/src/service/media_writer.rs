//! Адаптер черновика хранилища к `std::io::Write`: писатель DNG пишет потоком, а не одним буфером.

use std::io::{self, Write};

use shell_contract::still_media::PendingMedia;

/// Пишет байты в черновик через [`PendingMedia::write_all`]. Ошибка хранилища становится `io::Error`
/// с текстом платформы.
pub(super) struct PendingMediaWriter<'a> {
    draft: &'a mut dyn PendingMedia,
}

impl<'a> PendingMediaWriter<'a> {
    pub(super) fn new(draft: &'a mut dyn PendingMedia) -> Self {
        Self { draft }
    }
}

impl Write for PendingMediaWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.draft
            .write_all(bytes)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Черновик буферов не держит: каждый `write_all` уже ушёл платформе.
        Ok(())
    }
}
