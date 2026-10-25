//! Запись лога в форме, понятной logcat: уровень + текст. Чистая логика без FFI.

use std::ffi::CString;
use std::fmt::{self, Write as _};

use tracing::field::{Field, Visit};
use tracing::{Event, Level};

/// Максимум байт текста в одной записи logcat. Ядро режет запись на
/// `LOGGER_ENTRY_MAX_PAYLOAD` = 4068 байт вместе с тегом и служебными байтами;
/// 4000 оставляют запас под тег `Fasticamera` и уровень.
pub const LOGCAT_MAX_TEXT_BYTES: usize = 4000;

/// Уровень записи logcat (`android_LogPriority`). Числа — из `<android/log.h>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogPriority {
    Verbose = 2,
    Debug = 3,
    Info = 4,
    Warn = 5,
    Error = 6,
    /// Только для отчёта о панике: в logcat видна как `F`.
    Fatal = 7,
}

impl LogPriority {
    /// Соответствие уровней tracing и logcat: TRACE → VERBOSE, остальные один в один.
    pub fn from_tracing_level(level: &Level) -> Self {
        match *level {
            Level::TRACE => Self::Verbose,
            Level::DEBUG => Self::Debug,
            Level::INFO => Self::Info,
            Level::WARN => Self::Warn,
            Level::ERROR => Self::Error,
        }
    }

    /// Буква уровня как в выводе `adb logcat` — для приёмника stderr на ПК.
    pub fn letter(self) -> char {
        match self {
            Self::Verbose => 'V',
            Self::Debug => 'D',
            Self::Info => 'I',
            Self::Warn => 'W',
            Self::Error => 'E',
            Self::Fatal => 'F',
        }
    }
}

/// Готовая к записи строка лога.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub priority: LogPriority,
    pub text: String,
}

impl LogRecord {
    pub fn new(priority: LogPriority, text: String) -> Self {
        Self { priority, text }
    }

    /// Превращает событие tracing в запись вида `цель: сообщение поле=значение ...`.
    pub fn from_event(event: &Event<'_>) -> Self {
        let metadata = event.metadata();
        let mut visitor = EventTextVisitor::default();
        event.record(&mut visitor);
        Self {
            priority: LogPriority::from_tracing_level(metadata.level()),
            text: visitor.into_text(metadata.target()),
        }
    }

    /// Куски текста, каждый не длиннее `max_bytes` и разрезанный по границе символа UTF-8.
    /// Пустой текст — один пустой кусок: пустая запись лога тоже запись.
    pub fn chunks(&self, max_bytes: usize) -> Vec<&str> {
        split_at_char_boundaries(&self.text, max_bytes)
    }
}

/// Режет строку на куски ≤ `max_bytes` байт, не разрывая символы UTF-8.
/// `max_bytes` меньше 4 могло бы не вместить символ — тогда кусок берёт хотя бы один символ.
pub fn split_at_char_boundaries(text: &str, max_bytes: usize) -> Vec<&str> {
    if text.is_empty() {
        return vec![text];
    }
    let mut pieces = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut cut = rest.len().min(max_bytes);
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            // Символ шире лимита: отдаём его целиком, иначе цикл не продвинется.
            cut = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (piece, tail) = rest.split_at(cut);
        pieces.push(piece);
        rest = tail;
    }
    pieces
}

/// C-строка для FFI. Внутренний NUL обрезал бы текст в logcat молча, поэтому заменяем его
/// на видимый `\0`.
pub fn to_c_text(text: &str) -> CString {
    let without_nul = text.replace('\0', "\\0");
    // После замены NUL внутри нет, поэтому ветка ошибки недостижима; пустая строка вместо
    // паники — чтобы логирование никогда не роняло процесс.
    CString::new(without_nul).unwrap_or_default()
}

/// Собирает текст события: поле `message` — основным текстом, остальные — `имя=значение`.
#[derive(Default)]
struct EventTextVisitor {
    message: String,
    fields: String,
}

impl EventTextVisitor {
    fn into_text(self, target: &str) -> String {
        let mut text = format!("{target}: {}", self.message);
        if !self.fields.is_empty() {
            text.push(' ');
            text.push_str(&self.fields);
        }
        text
    }

    fn push_field(&mut self, field: &Field, value: fmt::Arguments<'_>) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
        // Запись в String не может завершиться ошибкой.
        let _ = write!(self.fields, "{}={}", field.name(), value);
    }
}

impl Visit for EventTextVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.push_field(field, format_args!("{value:?}"));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            // `tracing::info!("x {}", y)` приходит как Debug от fmt::Arguments — печатается без кавычек.
            let _ = write!(self.message, "{value:?}");
        } else {
            self.push_field(field, format_args!("{value:?}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_single_chunk() {
        assert_eq!(split_at_char_boundaries("abc", 10), vec!["abc"]);
    }

    #[test]
    fn empty_text_is_single_empty_chunk() {
        assert_eq!(split_at_char_boundaries("", 10), vec![""]);
    }

    #[test]
    fn long_text_is_split_without_breaking_utf8() {
        // «ж» — 2 байта; лимит 3 байта вмещает только один символ.
        let text = "жжж";
        let pieces = split_at_char_boundaries(text, 3);
        assert_eq!(pieces, vec!["ж", "ж", "ж"]);
        assert_eq!(pieces.concat(), text);
    }

    #[test]
    fn chunks_respect_limit_and_preserve_content() {
        let text = "a".repeat(LOGCAT_MAX_TEXT_BYTES * 2 + 17);
        let record = LogRecord::new(LogPriority::Info, text.clone());
        let pieces = record.chunks(LOGCAT_MAX_TEXT_BYTES);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.iter().all(|piece| piece.len() <= LOGCAT_MAX_TEXT_BYTES));
        assert_eq!(pieces.concat(), text);
    }

    #[test]
    fn char_wider_than_limit_still_progresses() {
        assert_eq!(split_at_char_boundaries("ж", 1), vec!["ж"]);
    }

    #[test]
    fn interior_nul_is_made_visible() {
        assert_eq!(to_c_text("a\0b").to_str().expect("utf8"), "a\\0b");
    }

    #[test]
    fn tracing_levels_map_to_logcat_priorities() {
        assert_eq!(LogPriority::from_tracing_level(&Level::TRACE), LogPriority::Verbose);
        assert_eq!(LogPriority::from_tracing_level(&Level::ERROR), LogPriority::Error);
        assert_eq!(LogPriority::Fatal as i32, 7);
    }
}
