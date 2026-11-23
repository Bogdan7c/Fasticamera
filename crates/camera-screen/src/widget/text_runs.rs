//! Кэш раскладки текста: подпись раскладывается один раз, а не на каждом кадре.
//!
//! Раскладка (`LabelPainter::layout`) выделяет память и шейпит текст. Статичные подписи ("EXPOSURE", "RAW",
//! "AE/AF LOCK", пункты меню) не меняются никогда, динамические (EV, секунды отсчёта) принимают конечный набор
//! значений. Поэтому достаточно запомнить `TextRun` по паре «текст + роль»: повторный кадр с тем же текстом
//! берёт готовый прогон, а перераскладка происходит только при **новом** тексте.
//!
//! Время жизни кэша — время жизни сцены: масштаб (`px_per_pt`) и токены типографики неизменны, пока сцена
//! жива; смена масштаба пересобирает сцену, а с ней и кэш. Предел размера задаёт конфиг
//! (`text_run_cache_capacity`): при переполнении кэш очищается целиком (счётчик `evictions`), а не растёт
//! бесконечно.

use std::collections::HashMap;

use ui_draw::text::TextRun;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::LabelPainter;

use super::Ctx;
use super::error::WidgetError;

/// Кэш прогонов текста. Ключ — роль, затем сам текст (вложенная карта позволяет искать по `&str` без
/// выделения `String` на каждом обращении).
#[derive(Debug)]
pub(crate) struct TextRuns {
    runs: HashMap<TextRoleKind, HashMap<String, TextRun>>,
    entries: usize,
    capacity: usize,
    /// Сколько раскладок реально выполнено (промахи кэша): по нему тесты и диагностика видят, что на
    /// повторных кадрах текст не раскладывается заново.
    layouts_performed: u64,
    /// Сколько раз кэш очищен из-за переполнения.
    evictions: u64,
}

impl TextRuns {
    /// Пустой кэш с пределом `capacity` разных подписей.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            runs: HashMap::new(),
            entries: 0,
            capacity,
            layouts_performed: 0,
            evictions: 0,
        }
    }

    /// Прогон текста: из кэша, а при промахе — раскладывается и запоминается.
    pub(crate) fn run(&mut self, ctx: &Ctx<'_>, text: &str, role: TextRoleKind) -> Result<&TextRun, WidgetError> {
        let cached = self.runs.get(&role).is_some_and(|by_text| by_text.contains_key(text));
        if !cached {
            // Раскладываем до изменения кэша: при ошибке кэш остаётся прежним.
            let run = LabelPainter::new(&ctx.tokens.typography, ctx.px).layout(text, role)?;
            self.layouts_performed += 1;
            if self.entries >= self.capacity {
                self.runs.clear();
                self.entries = 0;
                self.evictions += 1;
            }
            self.runs.entry(role).or_default().insert(text.to_owned(), run);
            self.entries += 1;
        }
        self.runs
            .get(&role)
            .and_then(|by_text| by_text.get(text))
            .ok_or_else(|| WidgetError::Inconsistent("прогон текста пропал из кэша сразу после вставки".to_owned()))
    }

    pub(crate) fn layouts_performed(&self) -> u64 {
        self.layouts_performed
    }

    pub(crate) fn evictions(&self) -> u64 {
        self.evictions
    }
}

/// Текст пользователя (имя пресета), пригодный для рисования: символы вне запечённого набора (иероглифы,
/// эмодзи) заменяются на «?». Имя вводит человек системной клавиатурой, а атлас — только ASCII, кириллица и
/// несколько знаков (`bake.json`); без замены один такой символ ронял бы кадр целиком (`TextError::UnsupportedCharacter`).
/// Замена — осознанная деградация вида, само имя в библиотеке пресетов не меняется.
pub(crate) fn displayable_text(ctx: &Ctx<'_>, text: &str) -> String {
    use ui_draw::text::TextError;
    use ui_kit::view::ViewError;

    let painter = LabelPainter::new(&ctx.tokens.typography, ctx.px);
    let mut chars: Vec<char> = text.chars().collect();
    // Каждая итерация чинит ровно один символ, поэтому больше `len` попыток не бывает.
    for _ in 0..=chars.len() {
        let candidate: String = chars.iter().collect();
        match painter.layout(&candidate, TextRoleKind::CardOption) {
            Err(ViewError::Text(TextError::UnsupportedCharacter { index, .. })) => match chars.get_mut(index) {
                Some(slot) => *slot = '?',
                None => break,
            },
            // Остальные ошибки (масштаб и т. п.) не про символы: их увидит сама раскладка при рисовании.
            _ => return candidate,
        }
    }
    chars.iter().collect()
}
