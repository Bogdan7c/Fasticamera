//! Перевод точек системы корня раздела в пиксели экрана для рисования без узла.
//!
//! Почти всё в меню рисуется от размещений узлов (`VisualIndex::placement`): поворот корня достаётся бесплатно.
//! Исключение — то, что не имеет своего узла: стекло панели (его рамку ведёт морф, а не сцена), «ручка» шторки,
//! заголовок и крупное значение карточки. Их положение задано точкой в системе корня (pt), а [`RootHost`] даёт
//! преобразование «система корня → экран» с учётом поворота корня по ориентации телефона.

use ui_core::geometry::Point;
use ui_core::id::NodeId;
use ui_draw::transform::Affine2;

use crate::widget::Ctx;
use crate::widget::layers::VisualIndex;

/// Преобразование системы корня раздела в пиксели экрана.
#[derive(Debug, Clone, Copy)]
pub(in crate::widget::calibration) struct RootHost {
    to_surface: Affine2,
}

impl RootHost {
    /// Хост по размещению корня; `root_center` — центр рамки корня (в этой точке корень вращается). `None`, пока
    /// у корня нет размещения (сцена ещё не разложена).
    pub(in crate::widget::calibration) fn of(
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        root: NodeId,
        root_center: Point,
    ) -> Option<Self> {
        let placement = visuals.placement(root)?;
        // Локальные пиксели размещения отсчитываются от центра рамки, а точки системы корня — от её начала.
        let from_origin = Affine2::translate(-ctx.px.px(root_center.x), -ctx.px.px(root_center.y));
        Some(Self {
            to_surface: from_origin.then(placement.transform),
        })
    }

    /// Всё преобразование «система корня → экран» (для стекла панели и «ручки»).
    pub(in crate::widget::calibration) fn transform(&self) -> Affine2 {
        self.to_surface
    }

    /// Преобразование точки `point` системы корня со сдвигом `slide_px` вдоль вертикали корня (въезд содержимого).
    pub(in crate::widget::calibration) fn point(&self, ctx: &Ctx<'_>, point: Point, slide_px: f32) -> Affine2 {
        Affine2::translate(ctx.px.px(point.x), ctx.px.px(point.y) + slide_px).then(self.to_surface)
    }
}
