//! Рисование внутри размещения узла раздела: смещения от центра рамки в pt, выравнивание текста, диски и кольца.
//!
//! Узлы раздела стоят в повёрнутом и прокручиваемом корне, поэтому каждый элемент ставится через
//! `placement.transform` узла (локальные пиксели от центра рамки → пиксели экрана): поворот и прокрутка
//! достаются бесплатно. Здесь собраны мелкие строители, чтобы виджеты не считали пиксели руками.

use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem, StrokeStyle};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::text::TextRun;
use ui_draw::transform::Affine2;
use ui_kit::view::{PxPerPt, centered_text};

use crate::widget::Ctx;
use crate::widget::error::WidgetError;

/// Кисть узла: его размещение, итоговая прозрачность и масштаб pt → px.
#[derive(Debug, Clone, Copy)]
pub(super) struct NodePaint {
    placement: NodePlacementPx,
    opacity: Opacity,
    px: PxPerPt,
}

impl NodePaint {
    /// Кисть узла с размещением `placement`; `extra_opacity` — множитель поверх прозрачности узла (затухание
    /// у края списка, «недоступная» ручка). `None` — узел невидим, рисовать нечего.
    pub(super) fn new(
        ctx: &Ctx<'_>,
        placement: NodePlacementPx,
        extra_opacity: f32,
    ) -> Result<Option<Self>, WidgetError> {
        let combined = (placement.opacity * extra_opacity).clamp(0.0, 1.0);
        if combined <= 0.0 {
            return Ok(None);
        }
        let opacity = Opacity::new(combined)
            .map_err(|_| WidgetError::Inconsistent("прозрачность узла раздела вне 0..1".to_owned()))?;
        Ok(Some(Self {
            placement,
            opacity,
            px: ctx.px,
        }))
    }

    /// Та же кисть, но прозрачнее в `factor` раз (приглушённая «↻»: как у карточки Exposure, затемнение — это
    /// прозрачность элемента, а не цвет).
    pub(super) fn dimmed(&self, factor: f32) -> Self {
        let dimmed = (self.opacity.get() * factor).clamp(0.0, 1.0);
        Self {
            opacity: Opacity::new(dimmed).unwrap_or(self.opacity),
            ..*self
        }
    }

    /// Размещение узла (для элементов `ui-kit`, которые ставятся сами).
    pub(super) fn placement(&self) -> NodePlacementPx {
        NodePlacementPx {
            opacity: self.opacity.get(),
            ..self.placement
        }
    }

    pub(super) fn opacity(&self) -> Opacity {
        self.opacity
    }

    /// Преобразование точки со смещением (`dx_pt`, `dy_pt`) от центра рамки узла.
    pub(super) fn at(&self, dx_pt: f64, dy_pt: f64) -> Affine2 {
        Affine2::translate(self.px.px(dx_pt), self.px.px(dy_pt)).then(self.placement.transform)
    }

    /// Текст, прижатый левым краем к `left_pt` (смещение от центра рамки), по вертикали центр на `y_pt`.
    pub(super) fn text_left(&self, run: &TextRun, left_pt: f64, y_pt: f64, color: DisplayColor) -> DrawItem {
        let half_advance_pt = f64::from(run.advance_px / self.px.get()) / 2.0;
        centered_text(run, self.at(left_pt + half_advance_pt, y_pt), color, self.opacity)
    }

    /// Текст, прижатый правым краем к `right_pt`.
    pub(super) fn text_right(&self, run: &TextRun, right_pt: f64, y_pt: f64, color: DisplayColor) -> DrawItem {
        let half_advance_pt = f64::from(run.advance_px / self.px.get()) / 2.0;
        centered_text(run, self.at(right_pt - half_advance_pt, y_pt), color, self.opacity)
    }

    /// Текст по центру точки (`x_pt`, `y_pt`).
    pub(super) fn text_centered(&self, run: &TextRun, x_pt: f64, y_pt: f64, color: DisplayColor) -> DrawItem {
        centered_text(run, self.at(x_pt, y_pt), color, self.opacity)
    }

    /// Закрашенный круг диаметра `diameter_pt` с центром в (`x_pt`, `y_pt`).
    pub(super) fn disc(&self, x_pt: f64, y_pt: f64, diameter_pt: f64, fill: DisplayColor) -> DrawItem {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle {
                diameter: self.px.px(diameter_pt),
            },
            transform: self.at(x_pt, y_pt),
            fill,
            stroke: None,
            shadow: None,
            opacity: self.opacity,
        })
    }

    /// Кольцо (круг без заливки с обводкой) диаметра `diameter_pt`.
    pub(super) fn ring(
        &self,
        (x_pt, y_pt): (f64, f64),
        diameter_pt: f64,
        width_pt: f64,
        color: DisplayColor,
    ) -> DrawItem {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Circle {
                diameter: self.px.px(diameter_pt),
            },
            transform: self.at(x_pt, y_pt),
            fill: DisplayColor::TRANSPARENT,
            stroke: Some(StrokeStyle {
                width_px: self.px.px(width_pt),
                color,
            }),
            shadow: None,
            opacity: self.opacity,
        })
    }

    /// Закрашенная капсула шириной `width_pt` и высотой `height_pt` с центром в (`x_pt`, `y_pt`).
    pub(super) fn capsule(
        &self,
        (x_pt, y_pt): (f64, f64),
        (width_pt, height_pt): (f64, f64),
        fill: DisplayColor,
    ) -> DrawItem {
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Capsule {
                size: PxSize::new(self.px.px(width_pt), self.px.px(height_pt)),
            },
            transform: self.at(x_pt, y_pt),
            fill,
            stroke: None,
            shadow: None,
            opacity: self.opacity,
        })
    }
}

/// Цвет из sRGB 8 бит на канал (цвета точек полос приходят от приложения именно так).
pub(super) fn srgb8(rgb: [u8; 3]) -> DisplayColor {
    let channel = |value: u8| f32::from(value) / 255.0;
    DisplayColor::rgb(channel(rgb[0]), channel(rgb[1]), channel(rgb[2]))
}
