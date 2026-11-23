//! Провод запроса текста между Rust и Kotlin (решение 197): коды и перевод в простые типы JNI и обратно.
//!
//! Контракт с Kotlin: те же числа — константы `TextInputDialog.PURPOSE_*` и `TextInputDialog.OUTCOME_*`
//! (`android/app/src/main/kotlin/dev/fasticamera/TextInputDialog.kt`).
//!
//! * Rust → Kotlin: [`TextRequestWire`] (`ShellPlatform.requestText(id: Long, purpose: Int, initialText: String,
//!   maxChars: Int)`);
//! * Kotlin → Rust: `ShellBridge.nativeTextInputResult(id: Long, outcome: Int, text: String?)`, разбирает
//!   [`decode_text_input_result`].
//!
//! Здесь только перевод формы. Нормализацию текста по длине запроса делает владелец очереди запросов
//! (`TextRequest::normalize_entry`): декодер запроса не знает.

use shell_contract::text_input::{TextInputOutcome, TextInputResult, TextPurpose, TextRequest, TextRequestId};

/// Назначение диалога для Kotlin (заголовок): `TextInputDialog.PURPOSE_*`.
pub fn text_purpose_code(purpose: TextPurpose) -> i32 {
    match purpose {
        TextPurpose::NewPresetName => 1,
        TextPurpose::RenamePreset => 2,
    }
}

/// Итог диалога: введено.
pub const OUTCOME_ENTERED: i32 = 1;
/// Итог диалога: отмена.
pub const OUTCOME_CANCELLED: i32 = 2;
/// Итог диалога: показать не удалось.
pub const OUTCOME_UNAVAILABLE: i32 = 3;

/// Запрос текста в форме аргументов JNI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRequestWire {
    /// Те же 64 бита номера запроса (`u64` → `long`).
    pub id: i64,
    pub purpose_code: i32,
    pub initial_text: String,
    /// Предел длины; больше `i32::MAX` символов Kotlin всё равно не примет, поэтому насыщение.
    pub max_chars: i32,
}

/// Переводит нейтральный запрос в аргументы вызова Kotlin.
pub fn encode_text_request(request: &TextRequest) -> TextRequestWire {
    TextRequestWire {
        id: i64::from_ne_bytes(request.id.0.to_ne_bytes()),
        purpose_code: text_purpose_code(request.purpose),
        initial_text: request.initial_text.clone(),
        max_chars: i32::try_from(request.max_chars).unwrap_or(i32::MAX),
    }
}

/// Ответ Kotlin не соответствует контракту.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextInputDecodeError {
    #[error("неизвестный код итога диалога: {0}")]
    UnknownOutcome(i32),
    #[error("итог «введено» пришёл без текста")]
    EnteredWithoutText,
}

/// Разбирает ответ Kotlin. Текст нужен только итогу «введено»; при отмене и «недоступно» он игнорируется.
pub fn decode_text_input_result(
    id: i64,
    outcome_code: i32,
    text: Option<String>,
) -> Result<TextInputResult, TextInputDecodeError> {
    let outcome = match outcome_code {
        OUTCOME_ENTERED => TextInputOutcome::Entered(text.ok_or(TextInputDecodeError::EnteredWithoutText)?),
        OUTCOME_CANCELLED => TextInputOutcome::Cancelled,
        OUTCOME_UNAVAILABLE => TextInputOutcome::Unavailable,
        unknown => return Err(TextInputDecodeError::UnknownOutcome(unknown)),
    };
    Ok(TextInputResult {
        id: TextRequestId(u64::from_ne_bytes(id.to_ne_bytes())),
        outcome,
    })
}

#[cfg(test)]
#[path = "text_input_wire_tests.rs"]
mod tests;
