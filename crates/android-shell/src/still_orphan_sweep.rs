//! Итог уборки осиротевших черновиков MediaStore (нейтральная часть, тестируется на ПК).
//!
//! Снимок, который писался в момент убийства процесса, оставляет в MediaStore скрытую запись
//! (`IS_PENDING = 1`): галерея её не видит, но она занимает место. При старте платформы Kotlin один раз на
//! процесс удаляет НАШИ такие записи, созданные до старта процесса (`StillMediaStore.startOrphanSweepOnce`).
//! Здесь — только язык ответа Kotlin → Rust: состояние уборки, а не голое число.

use serde::Serialize;

/// Ответ Kotlin «не запускалась» (платформа создана, уборку ещё не начали).
const TEXT_NOT_STARTED: &str = "not_started";
/// Ответ Kotlin «идёт на своём потоке».
const TEXT_RUNNING: &str = "running";
/// Начало ответа «закончена»: `removed:<число>`.
const PREFIX_REMOVED: &str = "removed:";
/// Начало ответа «упала»: `failed:<текст>`.
const PREFIX_FAILED: &str = "failed:";

/// Состояние уборки осиротевших черновиков.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OrphanSweepReport {
    /// Не запускалась: платформа не подключена (Activity нет) или уборка ещё не начата.
    NotStarted,
    /// Идёт на своём потоке Kotlin.
    Running,
    /// Закончена: удалено `count` осиротевших записей (0 — сирот не было).
    Removed { count: u32 },
    /// Упала; запись, которую не удалось удалить, могла остаться в хранилище.
    Failed { reason: String },
}

impl OrphanSweepReport {
    /// Разбирает ответ Kotlin. Непонятный ответ — `Failed` с текстом, а не молчаливое `NotStarted`.
    pub fn from_platform_text(text: &str) -> Self {
        let text = text.trim();
        if text == TEXT_NOT_STARTED {
            return Self::NotStarted;
        }
        if text == TEXT_RUNNING {
            return Self::Running;
        }
        if let Some(count_text) = text.strip_prefix(PREFIX_REMOVED) {
            return match count_text.parse::<u32>() {
                Ok(count) => Self::Removed { count },
                Err(_) => Self::unrecognized(text),
            };
        }
        if let Some(reason) = text.strip_prefix(PREFIX_FAILED) {
            return Self::Failed {
                reason: reason.trim().to_owned(),
            };
        }
        Self::unrecognized(text)
    }

    fn unrecognized(text: &str) -> Self {
        Self::Failed {
            reason: format!("неизвестный ответ платформы: `{text}`"),
        }
    }
}

#[cfg(test)]
#[path = "still_orphan_sweep_tests.rs"]
mod tests;
