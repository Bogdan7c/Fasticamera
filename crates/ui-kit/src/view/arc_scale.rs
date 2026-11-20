//! K3: вид дуговой шкалы («циферблат» колеса зума) — тёмный диск, кольцо рисок, подписи вдоль радиуса и
//! маркер текущего значения.
//!
//! Тот же `SnapScale`, что у линейки: механика (жесты, прилипание, события) не меняется, меняется только
//! вид. Шкала крутится под неподвижным маркером наверху: риска со смещением `offset` лежит под углом
//! `(offset − текущее) / pt_на_единицу × градусов_на_единицу` от маркера (по часовой — к большим значениям
//! при `FingerLeft`, как у линейки: контент едет за пальцем).
//!
//! # Что не знает вид
//! Какие значения подписывать и какие риски «яркие» (целые, объективы) — решает вызывающий: подписи
//! приходят готовыми ([`ArcLabel`]), яркость — функцией от значения. Камеры здесь нет.

use ui_core::time::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::text::TextRun;
use ui_draw::transform::Affine2;

use super::content::{centered_text, opacity_of_placement};
use super::error::ViewError;
use super::scale::PxPerPt;
use super::scale_ticks::growth_sign;
use crate::scale::{SnapScale, TickVisibility};
use crate::tokens::{ArcScaleTokens, ColorTokens};
use crate::value::{Emphasis, ValueModel};

/// Подпись у риски: текст уже свёрстан, положение — смещение риски на шкале, радиус — от центра диска.
#[derive(Debug, Clone)]
pub struct ArcLabel {
    /// Смещение подписанной риски в pt шкалы (как `ScaleMark::offset_points`).
    pub offset_points: f64,
    pub run: TextRun,
    pub color: DisplayColor,
    /// Радиус центра подписи от центра диска, pt.
    pub radius_pt: f64,
}

/// Элементы вида по слоям: подложка идёт под стекло, остальное — поверх.
#[derive(Debug, Clone, Default)]
pub struct ArcScaleDrawing {
    /// Тёмный диск (рисуется под всем интерфейсом колеса).
    pub backing: Vec<DrawItem>,
    /// Риски, подписи и маркер (рисуются поверх).
    pub foreground: Vec<DrawItem>,
}

/// Вид дуговой шкалы.
///
/// * `placement` — размещение узла-хоста (поворот, сдвиг и прозрачность узла наследуются);
/// * `center_offset_pt` — центр диска относительно центра узла-хоста, pt (x вправо, y вниз);
/// * `is_bright` — «яркая» ли риска со значением `value` (иначе тусклая, `dim_tick_alpha`).
///
/// Риски за углом `fade_end_degrees` не рисуются, между `fade_start_degrees` и `fade_end_degrees`
/// гаснут плавно. Шкала с [`TickVisibility::Hidden`] даёт только диск и маркер.
#[allow(clippy::too_many_arguments)]
pub fn arc_scale_items<M: ValueModel + Clone>(
    placement: &NodePlacementPx,
    center_offset_pt: (f64, f64),
    scale: &SnapScale<M>,
    now: MonotonicNanos,
    px_per_pt: PxPerPt,
    tokens: &ArcScaleTokens,
    colors: &ColorTokens,
    labels: &[ArcLabel],
    is_bright: impl Fn(M::Value) -> bool,
) -> Result<ArcScaleDrawing, ViewError> {
    let node_opacity = opacity_of_placement(placement, "arc_scale")?;
    let to_disc_center = Affine2::translate(px_per_pt.px(center_offset_pt.0), px_per_pt.px(center_offset_pt.1))
        .then(placement.transform);
    let mut drawing = ArcScaleDrawing::default();

    // Диск: круг радиуса r в центре; прозрачность диска перемножена с прозрачностью узла.
    drawing.backing.push(DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle {
            diameter: px_per_pt.px(tokens.disc_radius_pt * 2.0),
        },
        transform: to_disc_center,
        fill: DisplayColor::BLACK,
        stroke: None,
        shadow: None,
        opacity: scaled_opacity(node_opacity, tokens.disc_alpha, "arc_scale_disc")?,
    }));

    let spec = scale.spec();
    let sign = f64::from(growth_sign(spec.direction));
    let current_offset = scale.snapshot(now).offset_points;
    let points_per_unit = scale.points_per_unit();
    let angle_of =
        |offset_points: f64| sign * (offset_points - current_offset) / points_per_unit * tokens.degrees_per_unit;

    if spec.ticks == TickVisibility::Visible {
        let tick_length_pt = tokens.tick_outer_radius_pt - tokens.tick_inner_radius_pt;
        let tick_center_radius_pt = (tokens.tick_outer_radius_pt + tokens.tick_inner_radius_pt) * 0.5;
        for mark in scale.marks() {
            let angle = angle_of(mark.offset_points);
            let fade = edge_fade(angle.abs(), tokens);
            if fade <= 0.0 {
                continue;
            }
            let value = scale.model().value_at(mark.offset_points / points_per_unit);
            let bright = mark.emphasis == Emphasis::Reference || is_bright(value);
            let color = if bright {
                colors.text_primary
            } else {
                colors.text_primary.with_alpha(tokens.dim_tick_alpha)
            };
            drawing.foreground.push(radial_rect(
                to_disc_center,
                angle,
                tick_center_radius_pt,
                (tokens.tick_thickness_pt, tick_length_pt),
                color,
                scaled_opacity(node_opacity, fade, "arc_scale_tick")?,
                px_per_pt,
            ));
        }
        for label in labels {
            let angle = angle_of(label.offset_points);
            let fade = edge_fade(angle.abs(), tokens);
            if fade <= 0.0 {
                continue;
            }
            // Подпись стоит «головой наружу»: сначала вынос на радиус вверх, потом поворот на угол.
            let host = Affine2::translate(0.0, -px_per_pt.px(label.radius_pt))
                .then(Affine2::rotate(angle.to_radians() as f32))
                .then(to_disc_center);
            drawing.foreground.push(centered_text(
                &label.run,
                host,
                label.color,
                scaled_opacity(node_opacity, fade, "arc_scale_label")?,
            ));
        }
    }

    // Маркер текущего значения: неподвижный, всегда наверху (угол 0).
    drawing.foreground.push(radial_rect(
        to_disc_center,
        0.0,
        tokens.marker_center_radius_pt,
        (tokens.marker_width_pt, tokens.marker_height_pt),
        colors.focus_yellow,
        node_opacity,
        px_per_pt,
    ));
    Ok(drawing)
}

/// Множитель затухания у края дуги: 1 до `fade_start`, 0 с `fade_end`, плавно (smoothstep) между ними.
fn edge_fade(abs_angle_degrees: f64, tokens: &ArcScaleTokens) -> f32 {
    let span = tokens.fade_end_degrees - tokens.fade_start_degrees;
    let progress = ((abs_angle_degrees - tokens.fade_start_degrees) / span).clamp(0.0, 1.0);
    let smooth = progress * progress * (3.0 - 2.0 * progress);
    (1.0 - smooth) as f32
}

/// Прозрачность узла × множитель; результат проверяется как `Opacity`.
fn scaled_opacity(node: Opacity, factor: f32, what: &'static str) -> Result<Opacity, ViewError> {
    let value = node.get() * factor;
    Opacity::new(value).map_err(|_| ViewError::InvalidOpacity { what, value })
}

/// Прямоугольник `size_pt` (ширина, высота), центр которого лежит на радиусе `radius_pt` вверх от центра
/// диска и повёрнут вместе с лучом на `angle_degrees` по часовой.
fn radial_rect(
    to_disc_center: Affine2,
    angle_degrees: f64,
    radius_pt: f64,
    size_pt: (f64, f64),
    color: DisplayColor,
    opacity: Opacity,
    px_per_pt: PxPerPt,
) -> DrawItem {
    DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size: PxSize::new(px_per_pt.px(size_pt.0), px_per_pt.px(size_pt.1)),
            corner: CornerSpec::circular(0.0),
        },
        transform: Affine2::translate(0.0, -px_per_pt.px(radius_pt))
            .then(Affine2::rotate(angle_degrees.to_radians() as f32))
            .then(to_disc_center),
        fill: color,
        stroke: None,
        shadow: None,
        opacity,
    })
}
