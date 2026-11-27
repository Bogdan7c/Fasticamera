//! Дополнительные факты строки стенда, которых нет у обычного замера цены (сессия 14o): паритет с эталоном, размер
//! картинки прибора, счётчик созданных пайплайнов.
//!
//! Всё это лежит в ОДНОМ значении с умолчанием «ничего»: обычная работа, как и прежде, не знает про них, а новые поля
//! строки не требуют правки каждой подставной работы в тестах.

use serde::Serialize;

use super::parity_report::ParityOutcome;

/// Размер картинки цветового прибора в пикселях, на котором мерила работа `scopes_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PictureSizePx {
    /// Ширина картинки, пиксели.
    pub width: u32,
    /// Высота картинки, пиксели.
    pub height: u32,
}

impl PictureSizePx {
    /// Запись «ширинаxвысота» для компактной строки результата.
    pub fn compact(self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

/// Дополнительные факты строки; пустое значение — «нечего добавить».
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct WorkloadExtras {
    /// Паритет GPU с CPU-эталоном (только работы `look_parity`).
    pub parity: Option<ParityOutcome>,
    /// Размер картинки прибора (только работы `scopes_*`): отчёт называет настоящий размер раздела.
    pub scope_picture: Option<PictureSizePx>,
    /// Сколько пайплайнов видоискателя создано ПОСЛЕ конструктора работы, то есть за прогон (прогрев, окно, хвост).
    /// Должно быть 0: компиляция варианта посреди сессии — остановка рендера (`saturation_bench.md`, 16a).
    pub pipelines_created_during_run: Option<u64>,
}
