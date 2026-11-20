//! Уровень: линия по центру кадра, повёрнутая против ПОЛНОГО крена телефона: она всегда лежит вдоль горизонта
//! мира, в портретной и в альбомной (`level.rs`: «линия рисуется повёрнутой на −крен»). Рисуется, только когда
//! уровень включён и показание — `Tilted` (плашмя и «нет данных» не рисуем).
//! Ровно — жёлтая линия: судится по отклонению от БЛИЖАЙШЕЙ оси (0°, ±90°, 180°), чтобы ровный горизонт в
//! альбомной тоже был «ровно».
//!
//! Дефект 12b: линия поворачивалась на отклонение от ближайшей оси, и в альбомной (крен ≈ 90°, отклонение ≈ 0)
//! стояла вдоль короткой стороны экрана — в мире вертикально, как будто телефон всё ещё портретный.

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
        let LevelReading::Tilted {
            roll_clockwise_degrees,
            deviation_degrees,
        } = ctx.state.level()
        else {
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
        // Линия, нарисованная в начале координат и повёрнутая против полного крена, затем унесённая в центр кадра.
        let line = Rect::from_xywh(
            -overlay.level_length_pt * 0.5,
            -overlay.level_thickness_pt * 0.5,
            overlay.level_length_pt,
            overlay.level_thickness_pt,
        );
        let DrawItem::Shape(mut shape) = rect_item(line, color, Opacity::OPAQUE, ctx.px) else {
            return Err(WidgetError::Inconsistent("линия уровня не фигура".to_owned()));
        };
        shape.transform = Affine2::rotate((-roll_clockwise_degrees).to_radians())
            .then(Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y)));
        layers.push(Layer::FrameOverlay, DrawItem::Shape(shape));
        Ok(())
    }
}
