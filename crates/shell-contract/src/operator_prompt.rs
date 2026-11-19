//! Подсказка владельцу на экране телефона (замена вибрации как сигнала «сделай что-то руками»).
//!
//! Smoke-сценарии, где человек должен вмешаться (закрыть объектив рукой, щёлкнуть лампой), раньше
//! подавали сигнал вибрацией. Владелец решил: по тексту на экране он реагирует точнее. Подсказка — это
//! *состояние* («сейчас на экране висит такая строка»), а не импульс: она остаётся, пока её не заменят
//! или не снимут ([`OperatorPromptService::clear`]).
//!
//! Текст формирует Rust (сценарий), платформа только показывает: Kotlin логики не содержит и по
//! [`OperatorPromptKind`] выбирает лишь цвет фона.

use serde::Serialize;

use crate::commands::CommandOutcome;

/// Смысл подсказки; платформа превращает его в цвет фона плашки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorPromptKind {
    /// «Приготовься»: действие скоро, пока ничего делать не нужно (отсчёт).
    Prepare,
    /// «Делай сейчас»: действие нужно выполнить именно в этот момент.
    Act,
    /// «Готово»: действие закончено, можно вернуться в исходное положение.
    Done,
    /// Справка без требования действовать (название фазы, «жди следующего шага»).
    Info,
}

impl OperatorPromptKind {
    /// Все виды — для таблиц и проверки уникальности кодов платформы.
    pub const ALL: [OperatorPromptKind; 4] = [
        OperatorPromptKind::Prepare,
        OperatorPromptKind::Act,
        OperatorPromptKind::Done,
        OperatorPromptKind::Info,
    ];

    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            OperatorPromptKind::Prepare => "prepare",
            OperatorPromptKind::Act => "act",
            OperatorPromptKind::Done => "done",
            OperatorPromptKind::Info => "info",
        }
    }
}

/// Что показать владельцу: крупная строка действия и мелкая строка пояснения.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperatorPrompt {
    kind: OperatorPromptKind,
    headline: String,
    detail: String,
}

impl OperatorPrompt {
    /// `headline` — крупный текст действия («ЩЁЛКНИ ЛАМПОЙ»), `detail` — пояснение («ступенька 3 из 6 · фото»).
    pub fn new(kind: OperatorPromptKind, headline: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            headline: headline.into(),
            detail: detail.into(),
        }
    }

    pub fn kind(&self) -> OperatorPromptKind {
        self.kind
    }

    pub fn headline(&self) -> &str {
        &self.headline
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Одна строка для журнала сценария и текстов нарушений: «[act] ЩЁЛКНИ ЛАМПОЙ — ступенька 1».
    pub fn describe(&self) -> String {
        format!("[{}] {} — {}", self.kind.name(), self.headline, self.detail)
    }
}

/// Сервис подсказок владельцу. Потребители (сценарии) тестируются с fake-реализацией.
///
/// Оба метода неблокирующие: итог — только «поставлено ли в очередь платформы». Если платформа потом
/// не смогла показать текст, придёт `ShellEvent::PlatformCommandFailed` (отказ не теряется молча).
pub trait OperatorPromptService: Send + Sync {
    /// Показывает подсказку; следующая подсказка заменяет предыдущую.
    fn show(&self, prompt: &OperatorPrompt) -> CommandOutcome;

    /// Убирает подсказку с экрана. Без показанной подсказки — безвредный no-op платформы.
    fn clear(&self) -> CommandOutcome;
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn kinds_are_unique_and_named_like_serde() {
        let names: HashSet<_> = OperatorPromptKind::ALL.iter().map(|kind| kind.name()).collect();
        assert_eq!(names.len(), OperatorPromptKind::ALL.len());
        for kind in OperatorPromptKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).expect("json"),
                serde_json::json!(kind.name())
            );
        }
    }

    #[test]
    fn prompt_keeps_text_and_kind_and_describes_itself() {
        let prompt = OperatorPrompt::new(OperatorPromptKind::Act, "ЩЁЛКНИ ЛАМПОЙ", "ступенька 1 из 6");
        assert_eq!(prompt.kind(), OperatorPromptKind::Act);
        assert_eq!(prompt.headline(), "ЩЁЛКНИ ЛАМПОЙ");
        assert_eq!(prompt.detail(), "ступенька 1 из 6");
        assert_eq!(prompt.describe(), "[act] ЩЁЛКНИ ЛАМПОЙ — ступенька 1 из 6");
    }
}
