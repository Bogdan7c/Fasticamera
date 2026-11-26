//! Надстройки линейки поверх обычных рисок: тусклые промежутки хода и «упор» — отдельная риска на заданном значении.
//!
//! Это только рисунок: ни действий, ни магнита, ни движения линейки. Что промежуток или упор значит (ручка за упором
//! не меняет картинку), знает экран, а не `ui-kit`. По умолчанию надстроек нет, и линейка рисуется как раньше.

use ui_core::time::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::DrawItem;
use ui_draw::scene_bridge::NodePlacementPx;

use super::content::opacity_of_placement;
use super::error::ViewError;
use super::ruler::bar_item;
use super::scale::PxPerPt;
use crate::drive::{GrowthDirection, ScreenAxis};
use crate::scale::{SnapScale, TickVisibility};
use crate::tokens::RulerTokens;
use crate::value::RulerAxis;

/// Допуск сравнения положения риски с границей промежутка, единиц модели (много меньше единицы, но больше ошибки
/// перевода доли `f32` в положение, ≈ 1e-5): риска, лежащая ровно на границе (упор на
/// целой единице шкалы), к промежутку не относится, а накопленная ошибка `f64` её туда не переносит.
const SPAN_EDGE_TOLERANCE_UNITS: f64 = 1e-4;

/// Промежуток хода линейки, где риски тусклее: `(from, to]` в единицах положения модели (граница `from` в промежуток
/// не входит, `to` входит). Границы можно задать в любом порядке.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimmedSpan {
    /// Одна граница промежутка, единицы положения модели.
    pub from_units: f64,
    /// Другая граница промежутка, единицы положения модели.
    pub to_units: f64,
    /// Во сколько раз яркость рисок промежутка меньше обычной (`0..=1`).
    pub alpha_factor: f32,
}

/// Промежуток хода в значениях модели: то, что отдаёт экран. [`crate::ruler::RulerControl`] сам переводит значения
/// в положения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimmedRange<Value> {
    /// Граница промежутка, в промежуток не входит.
    pub from: Value,
    /// Другая граница промежутка, входит.
    pub to: Value,
    /// Во сколько раз яркость рисок промежутка меньше обычной (`0..=1`).
    pub alpha_factor: f32,
}

/// Множитель яркости риски в положении `position_units`: произведение множителей всех промежутков, куда она попала
/// (1,0 — риска вне всех промежутков).
pub(super) fn dimmed_factor(spans: &[DimmedSpan], position_units: f64) -> f32 {
    spans
        .iter()
        .filter(|span| {
            let (low, high) = (span.from_units.min(span.to_units), span.from_units.max(span.to_units));
            position_units > low + SPAN_EDGE_TOLERANCE_UNITS && position_units <= high + SPAN_EDGE_TOLERANCE_UNITS
        })
        .map(|span| span.alpha_factor)
        .product()
}

/// Где на линейке стоит надстройка в положении `position_units`: ось, сдвиг вдоль оси, затухание и прозрачность узла.
#[derive(Debug, Clone, Copy)]
pub(super) struct OverlaySite {
    pub axis: ScreenAxis,
    /// Сдвиг центра надстройки от центра узла вдоль оси, px.
    pub along_px: f32,
    /// Затухание по расстоянию до текущего значения (как у рисок).
    pub fade: f32,
    pub opacity: Opacity,
}

/// Место надстройки в положении `position_units` (единицы модели) или `None`: рисок нет, надстройка за краем узла
/// или в зоне, где риски уже погасли. `what` — имя надстройки для ошибки непрозрачности узла.
pub(super) fn overlay_site<M: RulerAxis>(
    placement: &NodePlacementPx,
    scale: &SnapScale<M>,
    now: MonotonicNanos,
    px_per_pt: PxPerPt,
    tokens: &RulerTokens,
    (position_units, what): (f64, &'static str),
) -> Result<Option<OverlaySite>, ViewError> {
    let spec = scale.spec();
    if spec.ticks == TickVisibility::Hidden {
        return Ok(None);
    }
    let opacity = opacity_of_placement(placement, what)?;
    let axis = spec.direction.axis();
    let sign = match spec.direction {
        GrowthDirection::FingerLeft | GrowthDirection::FingerUp => 1.0,
        GrowthDirection::FingerRight | GrowthDirection::FingerDown => -1.0,
    };
    let half_extent_px = match axis {
        ScreenAxis::Horizontal => placement.size.width,
        ScreenAxis::Vertical => placement.size.height,
    } * 0.5;
    let along_pt = position_units * scale.points_per_unit() - scale.snapshot(now).offset_points;
    let along_px = sign * px_per_pt.px(along_pt);
    let fade = tokens.fade_factor(along_pt);
    if along_px.abs() > half_extent_px || fade <= 0.0 {
        return Ok(None);
    }
    Ok(Some(OverlaySite {
        axis,
        along_px,
        fade,
        opacity,
    }))
}

/// Вид риски-упора: толще и длиннее обычной, чтобы читалась среди рисок. Цвет и размеры — от экрана.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RulerStopTickStyle {
    /// Толщина вдоль оси линейки, pt.
    pub thickness_pt: f64,
    /// Длина поперёк оси, pt.
    pub length_pt: f64,
    /// Цвет риски (вместе с прозрачностью).
    pub color: DisplayColor,
}

/// Риска-упор в положении `position_units`. Затухает с расстоянием вместе с обычными рисками. `None` — упор вне видимой
/// части линейки.
pub fn ruler_stop_tick_item<M: RulerAxis>(
    placement: &NodePlacementPx,
    scale: &SnapScale<M>,
    now: MonotonicNanos,
    px_per_pt: PxPerPt,
    tokens: &RulerTokens,
    (position_units, style): (f64, RulerStopTickStyle),
) -> Result<Option<DrawItem>, ViewError> {
    let Some(site) = overlay_site(
        placement,
        scale,
        now,
        px_per_pt,
        tokens,
        (position_units, "ruler stop tick"),
    )?
    else {
        return Ok(None);
    };
    Ok(Some(bar_item(
        placement,
        site.axis,
        site.along_px,
        (style.thickness_pt, style.length_pt),
        style.color.with_alpha(style.color.a * site.fade),
        px_per_pt,
        site.opacity,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(from: f64, to: f64, alpha_factor: f32) -> DimmedSpan {
        DimmedSpan {
            from_units: from,
            to_units: to,
            alpha_factor,
        }
    }

    #[test]
    fn a_tick_is_dimmed_only_strictly_beyond_the_first_border_and_up_to_the_last() {
        let spans = [span(92.0, 200.0, 0.4)];
        assert_eq!(dimmed_factor(&spans, 80.0), 1.0, "до упора — обычная риска");
        assert_eq!(dimmed_factor(&spans, 92.0), 1.0, "риска на самом упоре не тускнеет");
        assert_eq!(
            dimmed_factor(&spans, 92.0 + 1e-9),
            1.0,
            "ошибка округления не переносит риску за упор"
        );
        assert_eq!(dimmed_factor(&spans, 100.0), 0.4);
        assert_eq!(dimmed_factor(&spans, 200.0), 0.4, "край шкалы входит");
        assert_eq!(dimmed_factor(&spans, -100.0), 1.0, "другая сторона не затронута");
    }

    #[test]
    fn borders_in_either_order_give_the_same_span_and_spans_multiply() {
        assert_eq!(dimmed_factor(&[span(-200.0, -92.0, 0.5)], -150.0), 0.5);
        assert_eq!(dimmed_factor(&[span(-92.0, -200.0, 0.5)], -150.0), 0.5);
        assert_eq!(dimmed_factor(&[span(0.0, 10.0, 0.5), span(5.0, 10.0, 0.5)], 7.0), 0.25);
        assert_eq!(dimmed_factor(&[], 7.0), 1.0, "без промежутков риска обычная");
    }
}
