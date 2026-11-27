//! Как рисуется диаграмма CIE: цвет следа, заливка локуса и данные для них (эталон `scope_render_cie.glsl`).
//!
//! # Цвет цветности
//! `ByChromaticity`: цветность точки (x, y) → XYZ с `Y = 1` → линейный свет ИЗМЕРЯЕМОГО охвата (`xyz_to_display`) →
//! отрицательные каналы в ноль (цветность вне охвата) → деление на наибольший канал (нормированная яркость) →
//! кодировка sRGB (передача экрана). Цвет считается в центре пикселя картинки, а не клетки: сетка в картинку переносится
//! плавно (решение 322), клетка и пиксель — один порядок величины.
//!
//! # Заливка
//! Внутри спектрального локуса — выпуклый многоугольник из рёбер ([`CieTraceStyle::locus_edges`], доли картинки, y вниз).
//! Покрытие пикселя — не «внутри / снаружи», а непрерывное `clamp(0,5 + расстояние до ближайшего ребра, 0, 1)`
//! в пикселях, как у линий сетки: один пиксель на границе не даёт скачка сверки GPU и CPU.

use scope_contract::CieDiagram;

use crate::cie_chart::{ChartProjection, ChartWindow, xy_from_chart};
use crate::cie_locus::spectral_locus;
use crate::cie_rule::CieRule;
use crate::cie_settings::{CieLocusFill, CieRenderSettings, CieTraceColour};

/// Наибольшее число рёбер многоугольника локуса (65 вершин спектра, запас до 72: блок шейдера).
pub const MAX_LOCUS_EDGES: usize = 72;

/// Цвет белого следа.
pub const CIE_WHITE: [f32; 3] = [1.0; 3];

/// Наименьшее `y`, при котором цветность даёт цвет: ниже цвет — белый (деление на `y`).
const MIN_COLOUR_Y: f32 = 1.0e-4;

/// Порог линейного участка кодировки sRGB.
const SRGB_LINEAR_LIMIT: f32 = 0.003_130_8;

/// Способ рисования диаграммы CIE вместе с данными.
#[derive(Debug, Clone, PartialEq)]
pub struct CieTraceStyle {
    /// Шкала.
    pub diagram: CieDiagram,
    /// Окно диаграммы.
    pub window: ChartWindow,
    /// Цвет следа.
    pub trace_colour: CieTraceColour,
    /// Заливка локуса.
    pub locus_fill: CieLocusFill,
    /// Непрозрачность заливки.
    pub fill_opacity: f32,
    /// XYZ → линейный свет измеряемого охвата.
    pub xyz_to_display: [[f32; 3]; 3],
    /// Рёбра многоугольника локуса: `[из.x, из.y, в.x, в.y]` в долях картинки (y вниз); внутренность — слева от ребра
    /// (расстояние положительно).
    pub locus_edges: Vec<[f32; 4]>,
}

impl CieTraceStyle {
    /// Способ рисования для правила `rule` и настроек показа `settings`.
    pub fn new(rule: &CieRule, settings: &CieRenderSettings) -> Self {
        let projection = ChartProjection::new(rule.diagram(), rule.window());
        Self {
            diagram: rule.diagram(),
            window: rule.window(),
            trace_colour: settings.trace_colour,
            locus_fill: settings.locus_fill,
            fill_opacity: settings.locus_fill_opacity,
            xyz_to_display: rule.xyz_to_display(),
            locus_edges: locus_edges(&projection),
        }
    }

    /// Цвет цветности точки картинки `(u, v)` (доли картинки, y вниз), кодировка экрана.
    pub fn chromaticity_colour(&self, u: f32, v: f32) -> [f32; 3] {
        let chart = [
            self.window.left + u * self.window.extent,
            self.window.top() - v * self.window.extent,
        ];
        let Some([x, y]) = xy_from_chart(self.diagram, chart) else {
            return CIE_WHITE;
        };
        if y.is_nan() || y <= MIN_COLOUR_Y {
            return CIE_WHITE;
        }
        // XYZ с Y = 1.
        let xyz = [x / y, 1.0, (1.0 - x - y) / y];
        let linear = self
            .xyz_to_display
            .map(|row| ((row[0] * xyz[0] + row[1] * xyz[1]) + row[2] * xyz[2]).max(0.0));
        let peak = linear[0].max(linear[1]).max(linear[2]);
        if peak.is_nan() || peak <= 0.0 {
            return CIE_WHITE;
        }
        linear.map(|channel| encode_srgb(channel / peak))
    }

    /// Цвет следа в точке `(u, v)`.
    pub fn trace_colour_at(&self, u: f32, v: f32) -> [f32; 3] {
        match self.trace_colour {
            CieTraceColour::White => CIE_WHITE,
            CieTraceColour::ByChromaticity => self.chromaticity_colour(u, v),
        }
    }

    /// Заливка пикселя с центром `point_px` на картинке `picture_px`: цвет и ПРЯМАЯ альфа (0 — заливки нет).
    pub fn fill_at(&self, point_px: [f32; 2], picture_px: [f32; 2]) -> ([f32; 3], f32) {
        if self.locus_fill == CieLocusFill::Outline {
            return ([0.0; 3], 0.0);
        }
        let coverage = self.locus_coverage(point_px, picture_px);
        if coverage <= 0.0 {
            return ([0.0; 3], 0.0);
        }
        let colour = self.chromaticity_colour(point_px[0] / picture_px[0], point_px[1] / picture_px[1]);
        (colour, coverage * self.fill_opacity)
    }

    /// Покрытие пикселя внутренностью локуса: `clamp(0,5 + min_ребро(расстояние), 0, 1)`.
    pub fn locus_coverage(&self, point_px: [f32; 2], picture_px: [f32; 2]) -> f32 {
        let mut nearest = f32::INFINITY;
        for edge in &self.locus_edges {
            let from = [edge[0] * picture_px[0], edge[1] * picture_px[1]];
            let to = [edge[2] * picture_px[0], edge[3] * picture_px[1]];
            let along = [to[0] - from[0], to[1] - from[1]];
            let length = along[0].hypot(along[1]);
            if length.is_nan() || length <= 0.0 {
                continue;
            }
            let cross = along[0] * (point_px[1] - from[1]) - along[1] * (point_px[0] - from[0]);
            nearest = nearest.min(cross / length);
        }
        if nearest.is_finite() {
            (0.5 + nearest).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Кодировка sRGB (передача экрана): линейный участок и степень 1/2,4.
fn encode_srgb(linear: f32) -> f32 {
    if linear <= SRGB_LINEAR_LIMIT {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Рёбра многоугольника спектрального локуса, ориентированные так, что внутренность — слева (расстояние > 0).
pub fn locus_edges(projection: &ChartProjection) -> Vec<[f32; 4]> {
    let points: Vec<[f32; 2]> = spectral_locus()
        .into_iter()
        .map(|chromaticity| projection.picture_point(chromaticity))
        .collect();
    let count = points.len();
    let mut edges: Vec<[f32; 4]> = (0..count)
        .map(|index| {
            let (from, to) = (points[index], points[(index + 1) % count]);
            [from[0], from[1], to[0], to[1]]
        })
        .collect();
    // Ориентация по центру тяжести вершин (многоугольник выпуклый): если он справа от ребра — разворачиваем все рёбра.
    let centre = [
        points.iter().map(|point| point[0]).sum::<f32>() / count as f32,
        points.iter().map(|point| point[1]).sum::<f32>() / count as f32,
    ];
    let first = edges[0];
    let cross = (first[2] - first[0]) * (centre[1] - first[1]) - (first[3] - first[1]) * (centre[0] - first[0]);
    if cross < 0.0 {
        for edge in &mut edges {
            *edge = [edge[2], edge[3], edge[0], edge[1]];
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use isp_model::image::DisplayGamut;

    use super::*;
    use crate::cie_settings::CieSettings;

    fn style(diagram: CieDiagram, render: &CieRenderSettings) -> CieTraceStyle {
        let rule = CieRule::new(diagram, DisplayGamut::DisplayP3, &CieSettings::default()).unwrap();
        CieTraceStyle::new(&rule, render)
    }

    #[test]
    fn the_white_point_colour_is_white_and_a_green_point_is_green_dominant() {
        let render = CieRenderSettings {
            trace_colour: CieTraceColour::ByChromaticity,
            ..CieRenderSettings::default()
        };
        let xy = style(CieDiagram::Xy1931, &render);
        let projection = ChartProjection::new(CieDiagram::Xy1931, xy.window);
        let white = projection.picture_point(isp_model::color::chromaticity::Chromaticity::D65);
        let colour = xy.trace_colour_at(white[0], white[1]);
        assert!(colour.iter().all(|channel| (channel - 1.0).abs() < 2e-3), "{colour:?}");
        let green = projection.picture_point(isp_model::color::chromaticity::Chromaticity { x: 0.30, y: 0.60 });
        let colour = xy.trace_colour_at(green[0], green[1]);
        assert!(colour[1] > 0.999 && colour[0] < 0.9 && colour[2] < 0.6, "{colour:?}");
    }

    #[test]
    fn a_white_trace_ignores_the_chromaticity() {
        // Белый след — настройка (умолчание с решения 338 — цвет по цветности), поэтому задаётся явно.
        let white = CieRenderSettings {
            trace_colour: CieTraceColour::White,
            ..CieRenderSettings::default()
        };
        let xy = style(CieDiagram::Xy1931, &white);
        assert_eq!(xy.trace_colour_at(0.2, 0.7), CIE_WHITE);
    }

    #[test]
    fn the_locus_polygon_contains_the_white_point_and_excludes_the_corner_in_both_diagrams() {
        let render = CieRenderSettings {
            locus_fill: CieLocusFill::FaintColour,
            ..CieRenderSettings::default()
        };
        for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
            let style = style(diagram, &render);
            assert_eq!(style.locus_edges.len(), 65);
            let projection = ChartProjection::new(diagram, style.window);
            let white = projection.picture_point(isp_model::color::chromaticity::Chromaticity::D65);
            let picture = [1000.0, 1000.0];
            let inside = style.fill_at([white[0] * 1000.0, white[1] * 1000.0], picture);
            assert!(
                (inside.1 - render.locus_fill_opacity).abs() < 1e-6,
                "{diagram:?}: {inside:?}"
            );
            let outside = style.fill_at([5.0, 995.0], picture);
            assert_eq!(outside.1, 0.0, "{diagram:?} левый нижний угол вне локуса");
            let outline_only = super::CieTraceStyle {
                locus_fill: CieLocusFill::Outline,
                ..style.clone()
            };
            assert_eq!(
                outline_only.fill_at([white[0] * 1000.0, white[1] * 1000.0], picture).1,
                0.0
            );
        }
    }

    #[test]
    fn the_whole_locus_lies_inside_both_default_windows_with_a_margin() {
        for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
            let style = style(diagram, &CieRenderSettings::default());
            for edge in &style.locus_edges {
                for coordinate in [edge[0], edge[1]] {
                    assert!((0.015..=0.985).contains(&coordinate), "{diagram:?}: {coordinate}");
                }
            }
        }
    }
}
