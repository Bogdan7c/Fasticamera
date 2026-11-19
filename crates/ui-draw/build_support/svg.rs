//! Строгий разборщик подмножества SVG для иконок и запекание иконки в SDF (решения 81, 88).
//!
//! Поддерживается ровно то, что встречается в наборах Phosphor и Lucide:
//! `<svg viewBox>`, `<path d>`, `<rect>`, `<circle>`, `<line>`, `<polyline>` и атрибуты
//! `fill`, `stroke`, `stroke-width`, `stroke-linecap`, `stroke-linejoin` (наследуются от `<svg>`).
//! Всё остальное — ошибка сборки с именем файла и элемента: молча потерять часть иконки нельзя.
//!
//! Геометрию путей (включая дуги `A`) разбирает `kurbo::BezPath::from_svg`, уплощает `kurbo::flatten`.

use super::error::{BakeError, BakeResult};
use super::manifest::IconKind;
use super::sdf::{GridSize, LineCap, Point, StrokePolyline, encode_field, fill_distance_field, stroke_distance_field};
use kurbo::{BezPath, Circle, PathEl, RoundedRect, Shape};

/// Допуск уплощения, текселей (как у глифов).
const FLATTEN_TOLERANCE_TEXELS: f64 = 0.02;
/// Допуск преобразования круга/скруглённого прямоугольника в кривые, единиц viewBox (много меньше пикселя).
const SHAPE_TO_PATH_TOLERANCE: f64 = 0.001;

/// Оформление, наследуемое от `<svg>` к элементам.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SvgStyle {
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub stroke_width: Option<f64>,
    pub line_cap: Option<LineCap>,
    pub line_join: Option<String>,
}

impl SvgStyle {
    /// Оформление элемента поверх унаследованного: заданное у элемента важнее.
    fn over(&self, inherited: &SvgStyle) -> SvgStyle {
        SvgStyle {
            fill: self.fill.clone().or_else(|| inherited.fill.clone()),
            stroke: self.stroke.clone().or_else(|| inherited.stroke.clone()),
            stroke_width: self.stroke_width.or(inherited.stroke_width),
            line_cap: self.line_cap.or(inherited.line_cap),
            line_join: self.line_join.clone().or_else(|| inherited.line_join.clone()),
        }
    }

    fn paints_fill(&self) -> bool {
        self.fill.as_deref().is_some_and(|value| value != "none")
    }

    fn paints_stroke(&self) -> bool {
        self.stroke.as_deref().is_some_and(|value| value != "none")
    }
}

/// Фигура иконки: путь в координатах viewBox и итоговое оформление.
#[derive(Debug, Clone)]
pub struct SvgShape {
    pub path: BezPath,
    pub style: SvgStyle,
    /// Имя элемента (`path`, `rect`…) — для сообщений об ошибках.
    pub element: &'static str,
}

/// Разобранная иконка.
#[derive(Debug, Clone)]
pub struct SvgIcon {
    /// Сторона квадратного viewBox, единицы SVG.
    pub view_box_side: f64,
    pub shapes: Vec<SvgShape>,
}

/// Ячейка иконки в атласе.
#[derive(Debug, Clone, PartialEq)]
pub struct IconCell {
    pub size_texels: u32,
    pub texels: Vec<u8>,
}

/// Один тег XML: имя, атрибуты и вид.
struct Tag {
    name: String,
    attributes: Vec<(String, String)>,
    kind: TagKind,
}

#[derive(PartialEq, Eq)]
enum TagKind {
    Open,
    SelfClosing,
    Close,
}

/// Разбирает SVG-иконку; `file` нужен только для сообщений об ошибках.
pub fn parse_svg(file: &str, text: &str) -> BakeResult<SvgIcon> {
    let fail = |message: String| BakeError::new(format!("{file}: {message}"));
    let mut icon: Option<SvgIcon> = None;
    let mut root_style = SvgStyle::default();
    let mut depth = 0usize;
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after
                .split_once("-->")
                .map(|(_, tail)| tail)
                .ok_or_else(|| fail("незакрытый комментарий".into()))?;
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest
                .split_once('>')
                .map(|(_, tail)| tail)
                .ok_or_else(|| fail("незакрытая декларация".into()))?;
            continue;
        }
        let (tag, tail) = read_tag(rest).map_err(&fail)?;
        rest = tail;
        match (tag.kind == TagKind::Close, tag.name.as_str()) {
            (true, _) => depth = depth.saturating_sub(1),
            (false, "svg") => {
                if icon.is_some() || depth != 0 {
                    return Err(fail("вложенный <svg> не поддерживается".into()));
                }
                let (view_box_side, style) = read_root(&tag).map_err(&fail)?;
                root_style = style;
                icon = Some(SvgIcon {
                    view_box_side,
                    shapes: Vec::new(),
                });
                if tag.kind == TagKind::Open {
                    depth += 1;
                }
            }
            (false, name) => {
                let Some(icon) = icon.as_mut() else {
                    return Err(fail(format!("элемент <{name}> до <svg>")));
                };
                let shape = read_shape(&tag, &root_style).map_err(&fail)?;
                icon.shapes.push(shape);
                if tag.kind == TagKind::Open {
                    depth += 1;
                }
            }
        }
    }
    let icon = icon.ok_or_else(|| fail("нет элемента <svg>".into()))?;
    if icon.shapes.is_empty() {
        return Err(fail("в иконке нет ни одной фигуры".into()));
    }
    Ok(icon)
}

/// Читает один тег в начале `text` (текст начинается с `<`); возвращает тег и остаток после `>`.
fn read_tag(text: &str) -> Result<(Tag, &str), String> {
    let end = find_tag_end(text).ok_or_else(|| "незакрытый тег".to_string())?;
    let inner = &text[1..end];
    let tail = &text[end + 1..];
    if let Some(name) = inner.strip_prefix('/') {
        return Ok((
            Tag {
                name: name.trim().to_string(),
                attributes: Vec::new(),
                kind: TagKind::Close,
            },
            tail,
        ));
    }
    let (inner, kind) = match inner.strip_suffix('/') {
        Some(stripped) => (stripped, TagKind::SelfClosing),
        None => (inner, TagKind::Open),
    };
    let name_end = inner.find(|ch: char| ch.is_whitespace()).unwrap_or(inner.len());
    let name = inner[..name_end].to_string();
    let attributes = read_attributes(&inner[name_end..], &name)?;
    Ok((Tag { name, attributes, kind }, tail))
}

/// Позиция `>`, закрывающего тег, с учётом кавычек в значениях атрибутов.
fn find_tag_end(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in text.char_indices() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(open), _) if ch == open => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// Разбирает `имя="значение"` (кавычки двойные или одинарные).
fn read_attributes(text: &str, element: &str) -> Result<Vec<(String, String)>, String> {
    let mut attributes = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let (name, after_name) = rest
            .split_once('=')
            .ok_or_else(|| format!("элемент <{element}>: атрибут без значения «{rest}»"))?;
        let after_name = after_name.trim_start();
        let quote = after_name
            .chars()
            .next()
            .filter(|ch| *ch == '"' || *ch == '\'')
            .ok_or_else(|| format!("элемент <{element}>: значение атрибута «{}» без кавычек", name.trim()))?;
        let (value, after_value) = after_name[1..]
            .split_once(quote)
            .ok_or_else(|| format!("элемент <{element}>: незакрытая кавычка у «{}»", name.trim()))?;
        attributes.push((name.trim().to_string(), value.to_string()));
        rest = after_value.trim_start();
    }
    Ok(attributes)
}

/// Читает атрибуты `<svg>`: квадратный viewBox и оформление по умолчанию.
fn read_root(tag: &Tag) -> Result<(f64, SvgStyle), String> {
    let mut view_box_side = None;
    let mut style = SvgStyle::default();
    for (name, value) in &tag.attributes {
        match name.as_str() {
            "viewBox" => view_box_side = Some(parse_view_box(value)?),
            // Размер и пространство имён на запекание не влияют: масштаб задаёт манифест.
            "xmlns" | "width" | "height" => {}
            other => {
                if !read_style_attribute(other, value, &mut style)? {
                    return Err(format!("элемент <svg>: атрибут «{other}» не поддерживается"));
                }
            }
        }
    }
    let side = view_box_side.ok_or_else(|| "у <svg> нет viewBox".to_string())?;
    Ok((side, style))
}

/// `viewBox="0 0 W H"`: начало должно быть в нуле, фигура — квадрат.
fn parse_view_box(value: &str) -> Result<f64, String> {
    let numbers = parse_number_list(value)?;
    match numbers.as_slice() {
        [0.0, 0.0, width, height] if (width - height).abs() < 1e-9 && *width > 0.0 => Ok(*width),
        _ => Err(format!(
            "viewBox «{value}»: поддерживается только квадрат с началом в 0 0"
        )),
    }
}

/// Разбирает общие атрибуты оформления; `Ok(false)` — атрибут не из этой группы.
fn read_style_attribute(name: &str, value: &str, style: &mut SvgStyle) -> Result<bool, String> {
    match name {
        "fill" => style.fill = Some(value.to_string()),
        "stroke" => style.stroke = Some(value.to_string()),
        "stroke-width" => style.stroke_width = Some(parse_number(value)?),
        "stroke-linecap" => {
            style.line_cap = Some(match value {
                "butt" => LineCap::Butt,
                "round" => LineCap::Round,
                "square" => LineCap::Square,
                other => return Err(format!("stroke-linecap «{other}» не поддерживается")),
            });
        }
        "stroke-linejoin" => {
            // Генератор делает только круглые стыки; другие молча не подменяем.
            if value != "round" {
                return Err(format!("stroke-linejoin «{value}» не поддерживается (только round)"));
            }
            style.line_join = Some(value.to_string());
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Читает фигуру: геометрию по виду элемента и оформление поверх унаследованного.
fn read_shape(tag: &Tag, inherited: &SvgStyle) -> Result<SvgShape, String> {
    let element: &'static str = match tag.name.as_str() {
        "path" => "path",
        "rect" => "rect",
        "circle" => "circle",
        "line" => "line",
        "polyline" => "polyline",
        other => return Err(format!("элемент <{other}> не поддерживается")),
    };
    let mut style = SvgStyle::default();
    let mut geometry: Vec<(&str, &str)> = Vec::new();
    for (name, value) in &tag.attributes {
        if !read_style_attribute(name, value, &mut style)
            .map_err(|message| format!("элемент <{element}>: {message}"))?
        {
            if !geometry_attributes(element).contains(&name.as_str()) {
                return Err(format!("элемент <{element}>: атрибут «{name}» не поддерживается"));
            }
            geometry.push((name.as_str(), value.as_str()));
        }
    }
    let number = |name: &str, default: Option<f64>| -> Result<f64, String> {
        match geometry.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => {
                parse_number(value).map_err(|message| format!("элемент <{element}>, {name}: {message}"))
            }
            None => default.ok_or_else(|| format!("элемент <{element}>: нет атрибута «{name}»")),
        }
    };
    let text = |name: &str| -> Result<&str, String> {
        geometry
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
            .ok_or_else(|| format!("элемент <{element}>: нет атрибута «{name}»"))
    };
    let path = match element {
        "path" => BezPath::from_svg(text("d")?).map_err(|error| format!("элемент <path>: d: {error}"))?,
        "rect" => {
            let (x, y) = (number("x", Some(0.0))?, number("y", Some(0.0))?);
            let (width, height) = (number("width", None)?, number("height", None)?);
            // По SVG, если задан только один из rx/ry, второй равен ему.
            let (rx, ry) = match (number("rx", Some(-1.0))?, number("ry", Some(-1.0))?) {
                (rx, ry) if rx < 0.0 && ry < 0.0 => (0.0, 0.0),
                (rx, ry) if rx < 0.0 => (ry, ry),
                (rx, ry) if ry < 0.0 => (rx, rx),
                (rx, ry) => (rx, ry),
            };
            if (rx - ry).abs() > 1e-9 {
                return Err("элемент <rect>: rx ≠ ry (эллиптические углы) не поддерживаются".to_string());
            }
            RoundedRect::new(x, y, x + width, y + height, rx).to_path(SHAPE_TO_PATH_TOLERANCE)
        }
        "circle" => Circle::new((number("cx", Some(0.0))?, number("cy", Some(0.0))?), number("r", None)?)
            .to_path(SHAPE_TO_PATH_TOLERANCE),
        "line" => {
            let mut path = BezPath::new();
            path.move_to((number("x1", Some(0.0))?, number("y1", Some(0.0))?));
            path.line_to((number("x2", Some(0.0))?, number("y2", Some(0.0))?));
            path
        }
        _ => {
            let numbers =
                parse_number_list(text("points")?).map_err(|message| format!("элемент <polyline>: {message}"))?;
            if numbers.len() < 4 || numbers.len() % 2 != 0 {
                return Err("элемент <polyline>: points должен содержать не меньше двух точек".to_string());
            }
            let mut path = BezPath::new();
            for (index, pair) in numbers.chunks_exact(2).enumerate() {
                if index == 0 {
                    path.move_to((pair[0], pair[1]));
                } else {
                    path.line_to((pair[0], pair[1]));
                }
            }
            path
        }
    };
    Ok(SvgShape {
        path,
        style: style.over(inherited),
        element,
    })
}

/// Атрибуты геометрии, допустимые у каждого вида элемента.
fn geometry_attributes(element: &str) -> &'static [&'static str] {
    match element {
        "path" => &["d"],
        "rect" => &["x", "y", "width", "height", "rx", "ry"],
        "circle" => &["cx", "cy", "r"],
        "line" => &["x1", "y1", "x2", "y2"],
        _ => &["points"],
    }
}

fn parse_number(text: &str) -> Result<f64, String> {
    text.trim().parse::<f64>().map_err(|_| format!("«{text}» — не число"))
}

/// Список чисел через пробелы и запятые.
fn parse_number_list(text: &str) -> Result<Vec<f64>, String> {
    text.split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.is_empty())
        .map(parse_number)
        .collect()
}

/// Запекает иконку в квадратную ячейку `icon_texels + 2 · spread_texels`.
///
/// Согласованность с манифестом проверяется здесь: для `fill` у фигур должна быть заливка и не должно
/// быть штриха; для `stroke` — штрих без заливки и `stroke-width`, равный манифесту.
pub fn render_icon_cell(
    file: &str,
    icon: &SvgIcon,
    kind: IconKind,
    manifest_stroke_width: Option<f64>,
    icon_texels: u32,
    spread_texels: u32,
) -> BakeResult<IconCell> {
    let scale = f64::from(icon_texels) / icon.view_box_side;
    let spread = f64::from(spread_texels);
    let size_texels = icon_texels + 2 * spread_texels;
    let grid = GridSize {
        width: size_texels as usize,
        height: size_texels as usize,
    };
    let mut combined = vec![spread as f32; grid.width * grid.height];
    for shape in &icon.shapes {
        let element = shape.element;
        let subpaths = flatten_subpaths(&shape.path, FLATTEN_TOLERANCE_TEXELS / scale);
        let to_cell = |point: Point| Point::new(point.x * scale + spread, point.y * scale + spread);
        let field = match kind {
            IconKind::Fill => {
                if !shape.style.paints_fill() || shape.style.paints_stroke() {
                    return Err(BakeError::new(format!(
                        "{file}: <{element}>: для kind=fill нужна заливка и не нужен штрих"
                    )));
                }
                let contours: Vec<Vec<Point>> = subpaths
                    .iter()
                    .map(|(points, _)| points.iter().copied().map(to_cell).collect())
                    .collect();
                fill_distance_field(grid, &contours, spread)
            }
            IconKind::Stroke => {
                let expected = manifest_stroke_width
                    .ok_or_else(|| BakeError::new(format!("{file}: для kind=stroke нужен stroke_width в манифесте")))?;
                if shape.style.paints_fill() || !shape.style.paints_stroke() {
                    return Err(BakeError::new(format!(
                        "{file}: <{element}>: для kind=stroke нужен штрих и fill=\"none\""
                    )));
                }
                match shape.style.stroke_width {
                    Some(width) if (width - expected).abs() < 1e-9 => {}
                    other => {
                        return Err(BakeError::new(format!(
                            "{file}: <{element}>: stroke-width {other:?} не совпадает с манифестом ({expected})"
                        )));
                    }
                }
                let polylines: Vec<StrokePolyline> = subpaths
                    .iter()
                    .map(|(points, closed)| StrokePolyline {
                        points: points.iter().copied().map(to_cell).collect(),
                        closed: *closed,
                    })
                    .collect();
                let cap = shape.style.line_cap.unwrap_or(LineCap::Butt);
                stroke_distance_field(grid, &polylines, expected * scale * 0.5, cap, spread)
            }
        };
        // Объединение фигур иконки — минимум знаковых расстояний.
        for (slot, value) in combined.iter_mut().zip(field) {
            if value < *slot {
                *slot = value;
            }
        }
    }
    Ok(IconCell {
        size_texels,
        texels: encode_field(&combined, spread),
    })
}

/// Уплощает путь в ломаные: (точки, замкнута ли).
fn flatten_subpaths(path: &BezPath, tolerance: f64) -> Vec<(Vec<Point>, bool)> {
    let mut subpaths: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let flush = |subpaths: &mut Vec<(Vec<Point>, bool)>, current: &mut Vec<Point>, closed: bool| {
        let points = std::mem::take(current);
        if points.len() >= 2 {
            subpaths.push((points, closed));
        }
    };
    kurbo::flatten(path.elements().iter().copied(), tolerance, |element| match element {
        PathEl::MoveTo(target) => {
            flush(&mut subpaths, &mut current, false);
            current.push(Point::new(target.x, target.y));
        }
        PathEl::LineTo(target) => current.push(Point::new(target.x, target.y)),
        PathEl::ClosePath => flush(&mut subpaths, &mut current, true),
        // flatten выдаёт только линии; кривые сюда не приходят.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    flush(&mut subpaths, &mut current, false);
    subpaths
}
