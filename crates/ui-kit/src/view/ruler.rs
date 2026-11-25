//! Линейка шкалы нижней карточки iOS 27: равные риски, крупные ярче мелких, затухание у краёв.

use std::num::NonZeroU32;

use ui_core::time::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;

use super::content::opacity_of_placement;
use super::error::ViewError;
use super::scale::PxPerPt;
use crate::drive::{GrowthDirection, ScreenAxis};
use crate::scale::{SnapScale, TickVisibility};
use crate::tokens::{ColorTokens, RulerTokens};
use crate::value::RulerAxis;

/// Через сколько шагов шкалы повторяется крупная риска (для EV по 1/3 — каждый 3-й шаг, целый EV). «Шаг» —
/// расстояние между соседними рисками ([`RulerAxis::tick_step_units`]): у непрерывной оси это шаг её сетки.
/// `ui-kit` не знает, что такое EV: период задаёт экран.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MajorTickPeriod(NonZeroU32);

impl MajorTickPeriod {
    /// Крупная риска каждые `steps` шагов; ноль невозможен по типу.
    pub fn every_steps(steps: NonZeroU32) -> Self {
        Self(steps)
    }

    /// Крупная ли риска с номером шага `step_index` (от нуля модели, может быть отрицательным).
    fn is_major(self, step_index: i64) -> bool {
        step_index.rem_euclid(i64::from(self.0.get())) == 0
    }
}

/// Риски линейки и жёлтая отметка текущего значения. Устроено как [`super::scale_items`], но с видом
/// линейки: все риски одной длины, крупные (по [`MajorTickPeriod`]) ярче, яркость падает с
/// расстоянием до отметки ([`RulerTokens::fade_factor`]), риски дальше последнего звена затухания
/// не создаются. Отметка рисуется последней (поверх).
pub fn ruler_items<M: RulerAxis>(
    placement: &NodePlacementPx,
    scale: &SnapScale<M>,
    now: MonotonicNanos,
    px_per_pt: PxPerPt,
    tokens: &RulerTokens,
    major_period: MajorTickPeriod,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let spec = scale.spec();
    if spec.ticks == TickVisibility::Hidden {
        return Ok(Vec::new());
    }
    let opacity = opacity_of_placement(placement, "ruler")?;
    let axis = spec.direction.axis();
    let sign = match spec.direction {
        GrowthDirection::FingerLeft | GrowthDirection::FingerUp => 1.0,
        GrowthDirection::FingerRight | GrowthDirection::FingerDown => -1.0,
    };
    let half_extent_px = match axis {
        ScreenAxis::Horizontal => placement.size.width,
        ScreenAxis::Vertical => placement.size.height,
    } * 0.5;
    // «Шаг» линейки — расстояние между соседними рисками (у непрерывных осей это не единица положения).
    let points_per_step = scale.points_per_unit() * scale.model().tick_step_units();
    let current_offset = scale.snapshot(now).offset_points;

    let mut items = Vec::new();
    for mark in scale.marks() {
        let along_pt = mark.offset_points - current_offset;
        let along_px = sign * px_per_pt.px(along_pt);
        let fade = tokens.fade_factor(along_pt);
        if along_px.abs() > half_extent_px || fade <= 0.0 {
            continue;
        }
        let step_index = (mark.offset_points / points_per_step).round() as i64;
        let base_alpha = if major_period.is_major(step_index) {
            tokens.major_tick_alpha
        } else {
            tokens.minor_tick_alpha
        };
        let color = colors.text_primary.with_alpha(base_alpha * fade);
        items.push(bar_item(
            placement,
            axis,
            along_px,
            (tokens.tick_thickness_pt, tokens.tick_length_pt),
            color,
            px_per_pt,
            opacity,
        ));
    }
    items.push(bar_item(
        placement,
        axis,
        0.0,
        (tokens.current_mark_thickness_pt, tokens.current_mark_length_pt),
        colors.focus_yellow,
        px_per_pt,
        opacity,
    ));
    Ok(items)
}

/// Одна полоска: `(толщина вдоль оси, длина поперёк)` в pt, центр сдвинут на `along_px` от центра узла.
fn bar_item(
    placement: &NodePlacementPx,
    axis: ScreenAxis,
    along_px: f32,
    (thickness_pt, length_pt): (f64, f64),
    color: DisplayColor,
    px_per_pt: PxPerPt,
    opacity: Opacity,
) -> DrawItem {
    let thickness_px = px_per_pt.px(thickness_pt);
    let length_px = px_per_pt.px(length_pt);
    let (size, shift) = match axis {
        ScreenAxis::Horizontal => (PxSize::new(thickness_px, length_px), Affine2::translate(along_px, 0.0)),
        ScreenAxis::Vertical => (PxSize::new(length_px, thickness_px), Affine2::translate(0.0, along_px)),
    };
    DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size,
            corner: CornerSpec::circular(0.0),
        },
        transform: shift.then(placement.transform),
        fill: color,
        stroke: None,
        shadow: None,
        opacity,
    })
}
