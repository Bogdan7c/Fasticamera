//! Сетка и подписи диаграммы CIE (решения 332–333): спектральный локус с линией пурпурных, контуры охватов из настроек,
//! кружок белой точки и кривая чёрного тела — примитивы [`GraticulePrimitive`] в долях картинки (y вниз).
//!
//! Положения считаются через [`ChartProjection`] — ту же проекцию, что и цвет по месту и заливка локуса
//! ([`crate::cie_style`]), и то же окно, что и правило попадания: сетка не расходится со следом ни в одной шкале.
//!
//! # Подписи
//! Подпись охвата стоит снаружи его ЗЕЛЁНОЙ вершины (самая далёкая от белой): от белой точки по лучу через вершину на
//! `gamut_label_offset`, центр прижат к полю `label_margin`. Подписи разных охватов раздвигаются вниз, пока не разойдутся
//! на [`MIN_LABEL_SEPARATION`]. Подпись белой точки — справа-снизу от кружка.

use isp_model::color::chromaticity::Primaries;
use scope_contract::{ChromaticityGamut, LabelPoint, ScopeLabel, ScopeMark};

use crate::cie_chart::ChartProjection;
use crate::cie_locus::{planckian_chromaticity, spectral_locus};
use crate::cie_rule::CieRule;
use crate::cie_settings::CieMark;
use crate::graticule::GraticulePrimitive;
use crate::render_settings::RenderSettings;

/// Наименьшее расстояние между центрами подписей охватов, в долях картинки (две буквы «sRGB»/«P3» не слипаются).
pub const MIN_LABEL_SEPARATION: f32 = 0.07;
/// Температуры кривой чёрного тела, К: от тёплой свечи до холодного неба.
const PLANCKIAN_WARMEST_KELVIN: f64 = 1667.0;
const PLANCKIAN_COLDEST_KELVIN: f64 = 25_000.0;
/// Сколько раз пробуем сдвинуть подпись вниз, чтобы она разошлась с прежними.
const LABEL_NUDGE_ATTEMPTS: u32 = 8;

/// Основные цвета охвата для контура.
pub fn gamut_primaries(gamut: ChromaticityGamut) -> Primaries {
    match gamut {
        ChromaticityGamut::Srgb => Primaries::SRGB,
        ChromaticityGamut::DisplayP3 => Primaries::DISPLAY_P3,
        ChromaticityGamut::Rec2020 => Primaries::REC_2020,
    }
}

fn projection_of(rule: &CieRule) -> ChartProjection {
    ChartProjection::new(rule.diagram(), rule.window())
}

fn segment(from: [f32; 2], to: [f32; 2], half_width_px: f32) -> GraticulePrimitive {
    GraticulePrimitive::Segment {
        from,
        to,
        half_width_px,
    }
}

/// Замкнутая ломаная по вершинам.
fn closed_polyline(points: &[[f32; 2]], half_width_px: f32) -> Vec<GraticulePrimitive> {
    (0..points.len())
        .map(|index| segment(points[index], points[(index + 1) % points.len()], half_width_px))
        .collect()
}

/// Температуры кривой чёрного тела по равным шагам обратной температуры (миред): кривая гуще там, где цвет меняется.
fn planckian_temperatures(samples: u32) -> Vec<f64> {
    let (warm_mired, cold_mired) = (1.0e6 / PLANCKIAN_WARMEST_KELVIN, 1.0e6 / PLANCKIAN_COLDEST_KELVIN);
    (0..samples)
        .map(|index| {
            let fraction = f64::from(index) / f64::from(samples - 1);
            1.0e6 / (warm_mired + (cold_mired - warm_mired) * fraction)
        })
        .collect()
}

/// Сетка диаграммы: локус, затем контуры охватов в порядке настроек, затем белая точка, затем чёрное тело.
pub fn cie_graticule(rule: &CieRule, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
    let projection = projection_of(rule);
    let half_width_px = settings.graticule_half_width_px;
    let locus: Vec<[f32; 2]> = spectral_locus()
        .into_iter()
        .map(|chromaticity| projection.picture_point(chromaticity))
        .collect();
    let mut primitives = closed_polyline(&locus, half_width_px);
    for gamut in &settings.cie.reference_gamuts {
        let primaries = gamut_primaries(*gamut);
        let triangle = [primaries.red, primaries.green, primaries.blue].map(|vertex| projection.picture_point(vertex));
        primitives.extend(closed_polyline(&triangle, half_width_px));
    }
    if settings.cie.has_mark(CieMark::WhitePoint) {
        let radius = settings.cie.white_mark_radius;
        primitives.push(GraticulePrimitive::Ring {
            centre: white_point_on_picture(rule),
            radius: [radius, radius],
            half_width_px,
        });
    }
    if settings.cie.has_mark(CieMark::PlanckianLocus) {
        let curve: Vec<[f32; 2]> = planckian_temperatures(settings.cie.planckian_samples)
            .into_iter()
            .map(|kelvin| projection.picture_point(planckian_chromaticity(kelvin)))
            .collect();
        primitives.extend(curve.windows(2).map(|pair| segment(pair[0], pair[1], half_width_px)));
    }
    primitives
}

fn white_point_on_picture(rule: &CieRule) -> [f32; 2] {
    projection_of(rule).picture_point(rule.measured_gamut().primaries().white)
}

/// Подписи диаграммы: по одной на нарисованный контур охвата и подпись белой точки, если метка включена.
pub fn cie_labels(rule: &CieRule, settings: &RenderSettings) -> Vec<ScopeLabel> {
    let projection = projection_of(rule);
    let white = white_point_on_picture(rule);
    let margin = settings.cie.label_margin.min(0.5);
    let clamp_inside = |point: [f32; 2]| {
        [
            point[0].clamp(margin, 1.0 - margin),
            point[1].clamp(margin, 1.0 - margin),
        ]
    };
    let mut placed: Vec<[f32; 2]> = Vec::new();
    let mut labels = Vec::new();
    for gamut in &settings.cie.reference_gamuts {
        let vertex = projection.picture_point(gamut_primaries(*gamut).green);
        let from_white = [vertex[0] - white[0], vertex[1] - white[1]];
        let distance = from_white[0].hypot(from_white[1]);
        let outward = if distance > 0.0 {
            [from_white[0] / distance, from_white[1] / distance]
        } else {
            [0.0, -1.0]
        };
        let offset = settings.cie.gamut_label_offset;
        let mut point = clamp_inside([vertex[0] + outward[0] * offset, vertex[1] + outward[1] * offset]);
        for _ in 0..LABEL_NUDGE_ATTEMPTS {
            let crowded = placed
                .iter()
                .any(|other| (other[0] - point[0]).hypot(other[1] - point[1]) < MIN_LABEL_SEPARATION);
            if !crowded {
                break;
            }
            point = clamp_inside([point[0], point[1] + MIN_LABEL_SEPARATION]);
        }
        placed.push(point);
        if let Ok(at) = LabelPoint::try_new(point[0], point[1]) {
            labels.push(ScopeLabel {
                mark: ScopeMark::Gamut(*gamut),
                at,
            });
        }
    }
    if settings.cie.has_mark(CieMark::WhitePoint) {
        let reach = settings.cie.white_mark_radius + settings.cie.gamut_label_offset * 0.5;
        let point = clamp_inside([white[0] + reach, white[1] + reach]);
        if let Ok(at) = LabelPoint::try_new(point[0], point[1]) {
            labels.push(ScopeLabel {
                mark: ScopeMark::WhitePoint,
                at,
            });
        }
    }
    labels
}
