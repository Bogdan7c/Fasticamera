//! Ввод короткого текста через системную клавиатуру (решение 197): Rust просит, платформа спрашивает, ответ
//! приходит событием.
//!
//! Зачем так: вторичные экраны — Kotlin (решение 5), а системная клавиатура существует только у Android-вида
//! (`EditText` + IME). Поэтому Rust не рисует текстовое поле: он отправляет [`TextRequest`] («спроси имя»), Kotlin
//! показывает маленький диалог, а ответ возвращается событием `ShellEvent::TextInput(TextInputResult)`.
//!
//! # Протокол
//! * запрос получает [`TextRequestId`] от того, кто просит (монотонный счётчик на стороне Rust);
//! * на запрос приходит ровно ОДИН результат с тем же id: [`TextInputOutcome::Entered`], `Cancelled` (отмена,
//!   «назад», уход Activity на паузу) или `Unavailable` (диалог показать не удалось);
//! * пока диалог открыт, новый запрос ВЫТЕСНЯЕТ старый: старый получает `Superseded` (решает владелец очереди
//!   запросов — `TextInputBroker` в `fasticamera-app`, а не платформа), новый показывается. Результат вытесненного
//!   диалога, если платформа всё же его пришлёт, считается устаревшим и отбрасывается по несовпадению id;
//! * текст в ответе нормализуется по правилам [`TextRequest::normalize_entry`] (управляющие символы убираются,
//!   края обрезаются, длина ограничивается): платформе не доверяем даже при том, что Kotlin уже ограничил поле.
//!
//! Тексты интерфейса диалога (заголовок, кнопки) английские и живут в Kotlin; от Rust нужен только
//! [`TextPurpose`] — по нему Kotlin выбирает заголовок («New Preset» / «Rename Preset»).

use serde::Serialize;

use crate::commands::CommandOutcome;

/// Номер запроса текста. Выдаёт тот, кто просит; растёт монотонно, поэтому устаревший ответ отличим от текущего.
///
/// В JNI уходит как `long` те же 64 бита (`u64` ↔ `i64` без потерь), поэтому ограничений на значение нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TextRequestId(pub u64);

/// Для чего нужен текст. Нейтральный смысл: платформа по нему выбирает заголовок диалога, но не знает,
/// что Rust потом сделает с ответом.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPurpose {
    /// Имя нового пресета вида (заголовок «New Preset»).
    NewPresetName,
    /// Новое имя существующего пресета (заголовок «Rename Preset»).
    RenamePreset,
}

impl TextPurpose {
    /// Все назначения — для таблиц и проверки уникальности кодов платформы.
    pub const ALL: [TextPurpose; 2] = [TextPurpose::NewPresetName, TextPurpose::RenamePreset];

    /// Имя для логов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            TextPurpose::NewPresetName => "new_preset_name",
            TextPurpose::RenamePreset => "rename_preset",
        }
    }
}

/// Просьба спросить у владельца короткую строку.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextRequest {
    /// Номер запроса: ответ вернётся с ним же.
    pub id: TextRequestId,
    /// Для чего текст (выбирает заголовок диалога).
    pub purpose: TextPurpose,
    /// Что подставить в поле при показе (пустая строка — пустое поле).
    pub initial_text: String,
    /// Предел длины в символах (Unicode scalar values). Kotlin ограничивает ввод, Rust обрезает ответ.
    pub max_chars: u32,
}

impl TextRequest {
    /// Приводит ответ платформы к виду, пригодному для сохранения: убирает управляющие символы (в том числе
    /// переводы строк из вставки), обрезает пробелы по краям и ограничивает длину `max_chars` символами.
    ///
    /// Обрезка, а не отказ: пользователь вставил слишком длинную строку — лучше сохранить начало, чем потерять
    /// ввод; Kotlin ограничивает поле заранее, так что это защитный рубеж. `None` — после очистки не осталось
    /// ни одного символа (пустое имя сохранять нельзя).
    pub fn normalize_entry(&self, raw_text: &str) -> Option<String> {
        let without_controls: String = raw_text.chars().filter(|symbol| !symbol.is_control()).collect();
        let limit = usize::try_from(self.max_chars).unwrap_or(usize::MAX);
        // Сначала обрезаем длину, потом края: пробелы в конце обрезанной строки тоже не нужны.
        let truncated: String = without_controls.trim().chars().take(limit).collect();
        let normalized = truncated.trim_end();
        if normalized.is_empty() {
            None
        } else {
            Some(normalized.to_owned())
        }
    }
}

/// Чем кончился запрос текста.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", content = "text", rename_all = "snake_case")]
pub enum TextInputOutcome {
    /// Владелец ввёл и подтвердил текст («Save»).
    Entered(String),
    /// Владелец отказался (кнопка «Cancel», «назад») или Activity ушла с переднего плана.
    Cancelled,
    /// Диалог показать не удалось (нет Activity, исключение платформы): ввод невозможен, а не отменён.
    Unavailable,
    /// Диалог вытеснен более новым запросом (выдаётся владельцем очереди запросов, не платформой).
    Superseded,
}

/// Ответ на [`TextRequest`] с тем же номером.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextInputResult {
    pub id: TextRequestId,
    pub outcome: TextInputOutcome,
}

/// Команда оболочке «показать диалог ввода текста». Неблокирующая; ответ — событие `ShellEvent::TextInput`.
///
/// Итог — только «поставлена ли команда в очередь платформы» ([`CommandOutcome`]); если Activity нет или очередь
/// полна, диалога не будет и результата не придёт — вызывающий узнаёт об этом сразу из итога.
pub trait TextInputService: Send + Sync {
    /// Просит платформу показать диалог. Новый запрос заменяет открытый диалог без ответа по старому: учёт
    /// «кому какой ответ» ведёт вызывающий (`TextInputBroker`).
    fn request_text(&self, request: &TextRequest) -> CommandOutcome;
}

#[cfg(test)]
#[path = "text_input_tests.rs"]
mod tests;
