//! Вёрстка строки по запечённым метрикам Inter (решение 87).
//!
//! Владелец — пакет «запекание шрифта и иконок». Данные (продвижения, кернинг, рамки глифов в атласе)
//! сгенерированы `build.rs`; здесь — типы результата вёрстки, стиль и сама вёрстка.
//! Типы результата объявлены заранее: ими пользуется список рисования ([`crate::draw_list`]).

mod layout;
mod tables;
pub mod tracking;

use crate::atlas::AtlasRegion;
use crate::geometry::PxRect;

pub use layout::layout_line;
pub use tables::FontFace;

/// Один глиф свёрстанной строки: где он в атласе и куда ложится его прямоугольник.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedGlyph {
    /// Область SDF-атласа (с полем расстояний вокруг контура).
    pub region: AtlasRegion,
    /// Прямоугольник глифа в пикселях относительно начала строки; базовая линия — y = 0, вниз — плюс.
    pub quad: PxRect,
}

/// Свёрстанная строка: глифы и метрики для раскладки экрана.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// Глифы в порядке рисования (у пробела глифа нет — только продвижение).
    pub glyphs: Vec<PlacedGlyph>,
    /// Полная ширина строки (сумма продвижений с кернингом и трекингом), px.
    pub advance_px: f32,
    /// Высота заглавных, px: по ней экран выравнивает подписи по вертикали (`assets.md` 2).
    pub cap_height_px: f32,
}

/// Вертикальные метрики начертания в долях em.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetricsEm {
    /// Высота над базовой линией, em (положительная).
    pub ascent_em: f32,
    /// Глубина под базовой линией, em (отрицательная).
    pub descent_em: f32,
    /// Высота заглавных, em.
    pub cap_height_em: f32,
}

impl FontFace {
    /// Вертикальные метрики начертания; нужны раскладке, которой мало одного `cap_height_px`.
    pub fn metrics_em(self) -> FaceMetricsEm {
        let tables = self.tables();
        let units_per_em = f32::from(tables.units_per_em);
        FaceMetricsEm {
            ascent_em: tables.ascent_units as f32 / units_per_em,
            descent_em: tables.descent_units as f32 / units_per_em,
            cap_height_em: tables.cap_height_units as f32 / units_per_em,
        }
    }
}

/// Какие цифры использует строка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Figures {
    /// Цифры шрифта по умолчанию: у каждой своя ширина.
    Proportional,
    /// Табличные цифры (`tnum`): все одной ширины, подпись не дёргается при смене цифры.
    Tabular,
}

/// Стиль строки. Трекинг даёт вызывающий из токенов (см. [`tracking`] — стартовая таблица SF).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// Начертание.
    pub face: FontFace,
    /// Кегль: размер em в пикселях экрана.
    pub size_px: f32,
    /// Трекинг в долях em, добавляется после каждого глифа.
    pub tracking_em: f32,
    /// Набор цифр.
    pub figures: Figures,
}

/// Ошибка вёрстки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TextError {
    /// Символа нет в запечённом наборе; «тофу» вместо него не рисуем.
    #[error("символ {ch:?} (позиция {index} в строке, в символах) не входит в запечённый набор")]
    UnsupportedCharacter {
        /// Сам символ.
        ch: char,
        /// Номер символа в строке, считая от нуля (в символах Unicode, не в байтах).
        index: usize,
    },
}
