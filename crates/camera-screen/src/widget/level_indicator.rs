//! Уровень: линия по центру кадра, повёрнутая против крена телефона (горизонт остаётся горизонтом).
//! Рисуется, только когда уровень включён и показание — `Tilted` (плашмя и «нет данных» не рисуем).
//! Ровно (в пределах допуска) — жёлтая линия.

use ui_core::geometry::Rect;
use ui_draw::color::Opacity;
use ui_draw::draw_list::DrawItem;
use ui_draw::transform::Affine2;

use super::error::WidgetError;
use super::layers::{DrawLayers, Layer};
use super::{Ctx, rect_item};
use crate::level::LevelReading;
use crate::settings::LevelSetting;

pub(crate) struct LevelIndicatorWidget;

impl LevelIndicatorWidget {
    pub(crate) fn draw(ctx: &Ctx<'_>, layers: &mut DrawLayers) -> Result<(), WidgetError> {
        if ctx.state.settings().level != LevelSetting::On {
            return Ok(());
        }
        let LevelReading::Tilted { deviation_degrees, .. } = ctx.state.level() else {
            return Ok(());
        };
        let overlay = &ctx.geometry.config().overlay;
        let center = ctx.geometry.frame(ctx.state.settings().aspect).center();
        let aligned = deviation_degrees.abs() <= overlay.level_aligned_tolerance_deg;
        let color = if aligned {
            ctx.tokens.colors.focus_yellow
        } else {
            ctx.tokens.colors.text_primary
        };
        // Линия, нарисованная в начале координат и повёрнутая против крена, затем унесённая в центр кадра.
        let line = Rect::from_xywh(
            -overlay.level_length_pt * 0.5,
            -overlay.level_thickness_pt * 0.5,
            overlay.level_length_pt,
            overlay.level_thickness_pt,
        );
        let DrawItem::Shape(mut shape) = rect_item(line, color, Opacity::OPAQUE, ctx.px) else {
            return Err(WidgetError::Inconsistent("линия уровня не фигура".to_owned()));
        };
        shape.transform = Affine2::rotate((-deviation_degrees).to_radians())
            .then(Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y)));
        layers.push(Layer::FrameOverlay, DrawItem::Shape(shape));
        Ok(())
    }
}
