//! Манифест запекания `assets/bake.json`: что и как кладём в атлас (решения 81, 87, 88).
//!
//! Формат — JSON со строгим разбором (`deny_unknown_fields`): опечатка в имени поля — ошибка сборки.
//! Все пути — относительно папки `assets/`.

use super::error::{BakeError, BakeResult};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Корень манифеста.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeManifest {
    /// Файл шрифта (вариативный TTF) относительно `assets/`.
    pub font: String,
    /// Начертания: имя и координаты осей.
    pub faces: Vec<FaceSpec>,
    /// Какие символы кладём в атлас.
    pub charset: CharsetSpec,
    /// Возможности OpenType, которые выключаем при снятии метрик и кернинга (лигатуры, контекстные формы).
    pub disabled_features: Vec<String>,
    /// Параметры атласа.
    pub atlas: AtlasSpec,
    /// Иконки.
    pub icons: Vec<IconSpec>,
}

/// Одно начертание шрифта.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceSpec {
    /// Имя в `snake_case`; из него получается вариант `FontFace` (`semibold` → `Semibold`).
    pub name: String,
    /// Координаты осей в пользовательских единицах (`wght` 400, `opsz` 14).
    pub axes: BTreeMap<String, f32>,
}

/// Набор символов: диапазоны кодов плюс отдельные символы.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharsetSpec {
    /// Диапазоны кодов Unicode, границы включительно.
    pub ranges: Vec<CodeRange>,
    /// Отдельные символы строкой.
    pub extra: String,
}

/// Диапазон кодов Unicode, границы включительно.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeRange {
    pub from: u32,
    pub to: u32,
}

/// Параметры атласа.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtlasSpec {
    /// Ширина атласа, текселей; высота получается упаковкой.
    pub width_texels: u32,
    /// Размер em глифа в атласе, текселей.
    pub em_texels: u32,
    /// Поле расстояний вокруг формы: до скольких текселей от края хранится расстояние.
    pub spread_texels: u32,
    /// Сторона квадрата viewBox иконки в атласе, текселей (без поля расстояний).
    pub icon_texels: u32,
    /// Зазор между ячейками, текселей.
    pub gutter_texels: u32,
}

/// Иконка из исходного SVG.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconSpec {
    /// Имя в `snake_case`; из него получается вариант `IconId`.
    pub id: String,
    /// SVG-файл относительно `assets/`.
    pub file: String,
    /// Как рисовать: заливка или штрих.
    pub kind: IconKind,
    /// Ширина штриха в единицах viewBox (только для `stroke`).
    pub stroke_width: Option<f64>,
}

/// Вид иконки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconKind {
    /// Заливка контуров по ненулевой обмотке (Phosphor).
    Fill,
    /// Штрих по средней линии (Lucide).
    Stroke,
}

impl BakeManifest {
    /// Разбирает манифест из JSON и проверяет согласованность.
    pub fn parse(json: &str) -> BakeResult<Self> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|error| BakeError::new(format!("bake.json: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Проверяет значения, которые serde проверить не может.
    pub fn validate(&self) -> BakeResult<()> {
        if self.faces.is_empty() {
            return Err(BakeError::new("bake.json: нужно хотя бы одно начертание"));
        }
        ensure_unique_names("начертание", self.faces.iter().map(|face| face.name.as_str()))?;
        ensure_unique_names("иконка", self.icons.iter().map(|icon| icon.id.as_str()))?;
        for name in self
            .faces
            .iter()
            .map(|face| face.name.as_str())
            .chain(self.icons.iter().map(|icon| icon.id.as_str()))
        {
            ensure_snake_case(name)?;
        }
        if self.atlas.em_texels == 0 || self.atlas.spread_texels == 0 || self.atlas.icon_texels == 0 {
            return Err(BakeError::new(
                "bake.json: em_texels, spread_texels и icon_texels должны быть > 0",
            ));
        }
        for icon in &self.icons {
            match (icon.kind, icon.stroke_width) {
                (IconKind::Stroke, None) => {
                    return Err(BakeError::new(format!(
                        "bake.json: иконка {}: для stroke нужен stroke_width",
                        icon.id
                    )));
                }
                (IconKind::Stroke, Some(width)) if width <= 0.0 => {
                    return Err(BakeError::new(format!(
                        "bake.json: иконка {}: stroke_width должен быть > 0",
                        icon.id
                    )));
                }
                (IconKind::Fill, Some(_)) => {
                    return Err(BakeError::new(format!(
                        "bake.json: иконка {}: stroke_width не нужен для fill",
                        icon.id
                    )));
                }
                _ => {}
            }
        }
        for range in &self.charset.ranges {
            if range.from > range.to {
                return Err(BakeError::new("bake.json: диапазон символов: from > to"));
            }
        }
        Ok(())
    }

    /// Все символы набора по возрастанию кода, без повторов; пробел входит как обычный символ.
    pub fn characters(&self) -> BakeResult<Vec<char>> {
        let mut characters: Vec<char> = Vec::new();
        for range in &self.charset.ranges {
            for code in range.from..=range.to {
                let character = char::from_u32(code)
                    .ok_or_else(|| BakeError::new(format!("bake.json: код U+{code:X} не символ Unicode")))?;
                characters.push(character);
            }
        }
        characters.extend(self.charset.extra.chars());
        characters.sort_unstable();
        characters.dedup();
        Ok(characters)
    }
}

/// Имена должны быть уникальны: из них строятся варианты enum.
fn ensure_unique_names<'a>(kind: &str, names: impl Iterator<Item = &'a str>) -> BakeResult<()> {
    let mut seen: Vec<&str> = Vec::new();
    for name in names {
        if seen.contains(&name) {
            return Err(BakeError::new(format!("bake.json: {kind} «{name}» повторяется")));
        }
        seen.push(name);
    }
    Ok(())
}

/// Имя `snake_case`: строчные латинские буквы, цифры и `_`, начинается с буквы.
fn ensure_snake_case(name: &str) -> BakeResult<()> {
    let valid = name.chars().next().is_some_and(|first| first.is_ascii_lowercase())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(BakeError::new(format!(
            "bake.json: имя «{name}» должно быть snake_case (a-z, 0-9, _)"
        )))
    }
}

/// `snake_case` → `UpperCamelCase` для имён вариантов enum.
pub fn upper_camel_case(snake: &str) -> String {
    snake
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}
