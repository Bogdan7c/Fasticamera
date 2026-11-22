//! Фазы завершения снимка для экрана (решение 140: завершение в две фазы).
//!
//! «Кадр захвачен» освобождает затвор сразу после нажатия: следующий снимок можно делать, пока
//! предыдущий проявляется и пишется. «Сохранено» обновляет миниатюру. Ошибка возможна и до, и после
//! захвата — экран различает их, потому что после захвата затвор уже свободен.

use serde::Serialize;

use crate::develop::DevelopFailure;
use crate::frame_choice::NoFrameReason;
use crate::plan::StillId;

/// Где сорвался снимок.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum StillFailure {
    /// Очередь ждущих снимков полна (решение 140): нажатие отклонено, уже снятое не тронуто.
    #[error("очередь снимков полна")]
    QueueFull,
    /// Сервис снимка не работает (нет камеры, рендера или хранилища).
    #[error("сервис снимка недоступен")]
    ServiceUnavailable,
    /// Кадра нет.
    #[error("кадра для снимка нет: {0}")]
    NoFrame(NoFrameReason),
    /// Проявление на GPU.
    #[error("проявление: {0}")]
    Develop(String),
    /// Кодирование основной картинки.
    #[error("кодирование: {0}")]
    Encode(String),
    /// Запись в хранилище (MediaStore) или DNG.
    #[error("запись: {0}")]
    Store(String),
}

impl From<DevelopFailure> for StillFailure {
    fn from(failure: DevelopFailure) -> Self {
        Self::Develop(failure.to_string())
    }
}

/// Где лежат сохранённые файлы (адрес хранилища, например `content://media/...`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SavedStill {
    pub primary: String,
    /// DNG, если он был в плане.
    pub raw: Option<String>,
}

/// Фаза снимка, которую видит экран.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum StillPhase {
    /// Кадр захвачен: затвор свободен.
    Secured { id: StillId },
    /// Все файлы записаны и видны в галерее.
    Saved { id: StillId, files: SavedStill },
    /// Снимок сорвался; уже записанное не публикуется частично.
    Failed { id: StillId, failure: StillFailure },
}

impl StillPhase {
    pub fn id(&self) -> StillId {
        match self {
            Self::Secured { id } | Self::Saved { id, .. } | Self::Failed { id, .. } => *id,
        }
    }
}
