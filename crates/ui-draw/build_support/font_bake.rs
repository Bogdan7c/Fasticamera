//! Снятие данных шрифта: метрики, продвижения, кернинг и контуры глифов (решение 87).
//!
//! Работает только при сборке и в тестах: `harfrust` (вёрстка HarfBuzz) даёт продвижения, номера
//! глифов и пары кернинга, `skrifa` (Google Fontations) — контуры и метрики начертания.
//! В продукт не попадает ни один шрифтовой крейт: на телефоне остаются только таблицы.
//!
//! Кернинг снимается шейпингом пары: `kern(a, b) = ширина("ab") − ширина("a") − ширина("b")`.
//! Для шрифта без контекстных правил (они выключены в манифесте) этого достаточно, а тест сверяет
//! результат с полной вёрсткой HarfBuzz на настоящих подписях.

use super::error::{BakeError, BakeResult};
use super::manifest::FaceSpec;
use harfrust::{Buffer, Feature, Font as ShapingFont, GlyphId as ShapingGlyphId, ShapeOptions, ShaperFont, shape};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Цифры: у них есть табличный вариант (`tnum`).
pub const DIGITS: std::ops::RangeInclusive<char> = '0'..='9';

/// Какой набор цифр использует вёрстка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureStyle {
    /// Цифры шрифта по умолчанию (в Inter — пропорциональные).
    Proportional,
    /// Табличные цифры одной ширины (`tnum`).
    Tabular,
}

/// Общие метрики начертания, единицы шрифта.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetrics {
    pub units_per_em: u16,
    pub cap_height_units: i32,
    pub ascent_units: i32,
    pub descent_units: i32,
}

/// Символ с номером глифа и продвижением (без кернинга).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakedGlyph {
    pub character: char,
    pub glyph_id: u32,
    pub advance_units: i32,
}

/// Поправка продвижения первой буквы пары, единицы шрифта (отрицательная — сближает).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernPair {
    pub first: char,
    pub second: char,
    pub value_units: i32,
}

/// Команда контура глифа в единицах шрифта (ось y вверх).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCommand {
    MoveTo([f64; 2]),
    LineTo([f64; 2]),
    QuadTo([f64; 2], [f64; 2]),
    CubicTo([f64; 2], [f64; 2], [f64; 2]),
    Close,
}

/// Контур глифа как есть, в единицах шрифта.
pub type GlyphShape = Vec<PathCommand>;

/// Всё, что снято с одного начертания.
#[derive(Debug, Clone)]
pub struct FaceBake {
    pub name: String,
    pub metrics: FaceMetrics,
    /// Глифы с пропорциональными цифрами, по возрастанию символа.
    pub proportional: Vec<BakedGlyph>,
    /// Глифы с табличными цифрами, по возрастанию символа.
    pub tabular: Vec<BakedGlyph>,
    /// Пары кернинга для пропорциональных цифр, по возрастанию `(first, second)`.
    pub kerning_proportional: Vec<KernPair>,
    /// Пары кернинга для табличных цифр, по возрастанию `(first, second)`.
    pub kerning_tabular: Vec<KernPair>,
    /// Контуры по номеру глифа; у глифа без контура (пробел) записи нет.
    pub shapes: BTreeMap<u32, GlyphShape>,
}

/// Снимает данные начертания по манифесту.
pub fn bake_face(
    font_bytes: &[u8],
    spec: &FaceSpec,
    characters: &[char],
    disabled_features: &[String],
) -> BakeResult<FaceBake> {
    let shaping = ShapingSetup::new(font_bytes, spec, disabled_features)?;
    let proportional = shaping.baked_glyphs(characters, FigureStyle::Proportional)?;
    let tabular = shaping.baked_glyphs(characters, FigureStyle::Tabular)?;
    let kerning_proportional = shaping.kerning_pairs(characters, &proportional, FigureStyle::Proportional, None)?;
    // Табличные цифры меняют только пары с цифрой; остальные пары берём из пропорционального набора.
    let kerning_tabular =
        shaping.kerning_pairs(characters, &tabular, FigureStyle::Tabular, Some(&kerning_proportional))?;

    let outlines = OutlineSource::new(font_bytes, spec)?;
    let mut shapes = BTreeMap::new();
    for glyph in proportional.iter().chain(tabular.iter()) {
        if shapes.contains_key(&glyph.glyph_id) {
            continue;
        }
        let shape = outlines.shape_of(glyph.glyph_id, glyph.character)?;
        if !shape.is_empty() {
            shapes.insert(glyph.glyph_id, shape);
        }
    }
    Ok(FaceBake {
        name: spec.name.clone(),
        metrics: outlines.metrics()?,
        proportional,
        tabular,
        kerning_proportional,
        kerning_tabular,
        shapes,
    })
}

/// Шрифт, настроенный для шейпинга: нужное начертание и наборы возможностей.
pub struct ShapingSetup {
    font: ShapingFont,
    base_features: Vec<Feature>,
    tabular_features: Vec<Feature>,
}

impl ShapingSetup {
    /// Готовит шейпинг начертания; `disabled_features` — теги вида `calt`, выключаются значением 0.
    pub fn new(font_bytes: &[u8], spec: &FaceSpec, disabled_features: &[String]) -> BakeResult<Self> {
        let default_font =
            ShapingFont::new(font_bytes.to_vec(), 0).ok_or_else(|| BakeError::new("harfrust не смог открыть шрифт"))?;
        let font = default_font
            .instance_builder()
            .variations(spec.axes.iter().map(|(tag, value)| (tag.as_str(), *value)))
            .build();
        let base_features = disabled_features
            .iter()
            .map(|tag| parse_feature(&format!("-{tag}")))
            .collect::<BakeResult<Vec<_>>>()?;
        let mut tabular_features = base_features.clone();
        tabular_features.push(parse_feature("tnum")?);
        Ok(Self {
            font,
            base_features,
            tabular_features,
        })
    }

    /// Шейпит строку; возвращает (номер глифа, продвижение) каждого глифа.
    pub fn shape_text(&self, text: &str, figures: FigureStyle) -> BakeResult<Vec<(u32, i32)>> {
        let shaper_font = ShaperFont::new(&self.font);
        let mut buffer = Buffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let features = match figures {
            FigureStyle::Proportional => &self.base_features,
            FigureStyle::Tabular => &self.tabular_features,
        };
        shape(&shaper_font, &mut buffer, ShapeOptions::new().features(features))
            .map_err(|error| BakeError::new(format!("harfrust: шейпинг «{text}»: {error:?}")))?;
        Ok(buffer
            .glyph_infos()
            .iter()
            .zip(buffer.glyph_positions())
            .map(|(info, position)| (info.glyph_id, position.x_advance))
            .collect())
    }

    /// Номер глифа и продвижение каждого символа набора при заданном стиле цифр.
    fn baked_glyphs(&self, characters: &[char], figures: FigureStyle) -> BakeResult<Vec<BakedGlyph>> {
        characters
            .iter()
            .map(|&character| {
                let shaped = self.shape_text(&character.to_string(), figures)?;
                match shaped.as_slice() {
                    // Глиф .notdef (0) значит, что шрифт символа не знает — «тофу» в атлас не берём.
                    [(0, _)] => Err(BakeError::new(format!(
                        "шрифт не содержит символ {character:?} (U+{:04X})",
                        character as u32
                    ))),
                    [(glyph_id, advance)] => Ok(BakedGlyph {
                        character,
                        glyph_id: *glyph_id,
                        advance_units: *advance,
                    }),
                    other => Err(BakeError::new(format!(
                        "символ {character:?} дал {} глифов вместо одного: лигатуры и разложение не поддерживаются",
                        other.len()
                    ))),
                }
            })
            .collect()
    }

    /// Пары кернинга по всем парам символов набора. Если задан `reuse_from`, пары без цифр берутся оттуда.
    fn kerning_pairs(
        &self,
        characters: &[char],
        glyphs: &[BakedGlyph],
        figures: FigureStyle,
        reuse_from: Option<&[KernPair]>,
    ) -> BakeResult<Vec<KernPair>> {
        let mut pairs = Vec::new();
        for (first_index, first) in glyphs.iter().enumerate() {
            for (second_index, second) in glyphs.iter().enumerate() {
                let involves_digit = DIGITS.contains(&first.character) || DIGITS.contains(&second.character);
                if let (Some(source), false) = (reuse_from, involves_digit) {
                    if let Some(pair) = source
                        .iter()
                        .find(|pair| pair.first == first.character && pair.second == second.character)
                    {
                        pairs.push(*pair);
                    }
                    continue;
                }
                let text: String = [characters[first_index], characters[second_index]].iter().collect();
                let shaped = self.shape_text(&text, figures)?;
                let total_advance = match shaped.as_slice() {
                    [(_, left), (_, right)] => left + right,
                    other => {
                        return Err(BakeError::new(format!(
                            "пара {text:?} дала {} глифов вместо двух",
                            other.len()
                        )));
                    }
                };
                let value_units = total_advance - first.advance_units - second.advance_units;
                if value_units != 0 {
                    pairs.push(KernPair {
                        first: first.character,
                        second: second.character,
                        value_units,
                    });
                }
            }
        }
        pairs.sort_by_key(|pair| (pair.first, pair.second));
        Ok(pairs)
    }
}

/// Разбирает возможность OpenType в синтаксисе HarfBuzz (`-calt`, `tnum`).
fn parse_feature(text: &str) -> BakeResult<Feature> {
    Feature::from_str(text).map_err(|reason| BakeError::new(format!("возможность OpenType «{text}»: {reason}")))
}

/// Контуры и метрики начертания через `skrifa`.
struct OutlineSource<'font> {
    font: FontRef<'font>,
    location: skrifa::instance::Location,
}

impl<'font> OutlineSource<'font> {
    fn new(font_bytes: &'font [u8], spec: &FaceSpec) -> BakeResult<Self> {
        let font = FontRef::new(font_bytes).map_err(|error| BakeError::new(format!("skrifa: шрифт: {error}")))?;
        let location = font
            .axes()
            .location(spec.axes.iter().map(|(tag, value)| (tag.as_str(), *value)));
        Ok(Self { font, location })
    }

    fn metrics(&self) -> BakeResult<FaceMetrics> {
        let metrics = self.font.metrics(Size::unscaled(), &self.location);
        let cap_height = metrics
            .cap_height
            .ok_or_else(|| BakeError::new("в OS/2 шрифта нет высоты заглавных (sCapHeight)"))?;
        Ok(FaceMetrics {
            units_per_em: metrics.units_per_em,
            cap_height_units: cap_height.round() as i32,
            ascent_units: metrics.ascent.round() as i32,
            descent_units: metrics.descent.round() as i32,
        })
    }

    fn shape_of(&self, glyph_id: u32, character: char) -> BakeResult<GlyphShape> {
        let outlines = self.font.outline_glyphs();
        let Some(glyph) = outlines.get(GlyphId::new(glyph_id)) else {
            // Глиф без контура (пробел) — не ошибка, у него просто нет картинки.
            return Ok(Vec::new());
        };
        let mut pen = CollectingPen::default();
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::from(&self.location)),
                &mut pen,
            )
            .map_err(|error| BakeError::new(format!("skrifa: контур {character:?}: {error}")))?;
        Ok(pen.commands)
    }
}

/// Перо, складывающее команды контура в список.
#[derive(Default)]
struct CollectingPen {
    commands: Vec<PathCommand>,
}

fn point(x: f32, y: f32) -> [f64; 2] {
    [f64::from(x), f64::from(y)]
}

impl OutlinePen for CollectingPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCommand::MoveTo(point(x, y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(PathCommand::LineTo(point(x, y)));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.commands.push(PathCommand::QuadTo(point(cx0, cy0), point(x, y)));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands
            .push(PathCommand::CubicTo(point(cx0, cy0), point(cx1, cy1), point(x, y)));
    }
    fn close(&mut self) {
        self.commands.push(PathCommand::Close);
    }
}

/// Тип номера глифа harfrust совпадает с `skrifa` (оба — из `read-fonts`); проверка на этапе компиляции.
#[allow(dead_code)]
fn glyph_id_types_match(id: ShapingGlyphId) -> GlyphId {
    id
}
