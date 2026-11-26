//! Слои списка рисования и индекс видимых узлов сцены.
//!
//! Порядок слоёв (`glass.md`: панель не на панели, содержимое поверх панели):
//! подложка вне кадра → сетка/уровень/затемнение кадра → плоское под панелями (рамка фокуса, плашка) →
//! панели (поверхности PHOTO и раздела; имя слоя `Glass` осталось с тех пор, когда панели были стеклом) →
//! содержимое (иконки, подписи, диск затвора) → шторка меню целиком (её панель, затем её содержимое).
//!
//! Панель (шторка меню, панель Exposure) лежит над всей нижней зоной: затвор, капсула режимов и кнопки уходят
//! под неё (решение 94 для шторки), поэтому даже растворяющийся диск затвора не должен оказаться поверх панели.
//! Это достигается порядком рисования: каждая панель собирается в отдельный набор слоёв и кладётся поверх
//! остальных (`stack_on_top`). С 14p панели полупрозрачны без размытия, поэтому нижний ряд под ними ещё и
//! растворяется (`ScreenState::is_bottom_row_hidden`), а не просвечивает.

use std::collections::HashMap;

use ui_core::id::NodeId;
use ui_core::scene::UiScene;
use ui_core::tree::NodeVisual;
use ui_draw::draw_list::{ClipGroup, DrawItem};
use ui_draw::geometry::PxRect;
use ui_draw::scene_bridge::{NodePlacementPx, node_placement_px};

use super::intent::ScreenGesture;

/// Слой списка рисования.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layer {
    /// Фон вне кадра (поверх видоискателя, под всем остальным).
    Backdrop,
    /// Сетка, уровень, затемнение при снимке: поверх кадра, под интерфейсом.
    FrameOverlay,
    /// Плоское, что стоит под панелями (рамка фокуса, плашка).
    Plain,
    /// Панели: поверхности PHOTO и раздела (до 14p — группы стекла, отсюда имя).
    Glass,
    /// Содержимое поверх панелей и прочий интерфейс сверху.
    Content,
}

/// Собранные элементы по слоям.
#[derive(Debug, Default)]
pub(crate) struct DrawLayers {
    backdrop: Vec<DrawItem>,
    frame_overlay: Vec<DrawItem>,
    plain: Vec<DrawItem>,
    glass: Vec<DrawItem>,
    content: Vec<DrawItem>,
    /// Стекло шторки меню и всё, что лежит под её содержимым: поверх всего обычного интерфейса.
    overlay_glass: Vec<DrawItem>,
    /// Содержимое шторки меню: поверх её стекла.
    overlay_content: Vec<DrawItem>,
}

impl DrawLayers {
    pub(crate) fn push(&mut self, layer: Layer, item: DrawItem) {
        self.layer_mut(layer).push(item);
    }

    pub(crate) fn extend(&mut self, layer: Layer, items: impl IntoIterator<Item = DrawItem>) {
        self.layer_mut(layer).extend(items);
    }

    /// Элементы `items` слоя видны только внутри прямоугольника экрана `window` (px). Пустой набор не кладётся.
    pub(crate) fn push_clipped(&mut self, layer: Layer, window: PxRect, items: Vec<DrawItem>) {
        if !items.is_empty() {
            self.push(layer, DrawItem::Clipped(ClipGroup { rect: window, items }));
        }
    }

    /// Всё, что нарисует `draw`, будет видно только внутри прямоугольника экрана `window` (px): окно прокручиваемого
    /// содержимого. Слои сохраняются: в каждый непустой слой ложится одна группа обрезки на месте своего содержимого.
    pub(crate) fn clipped_to<E>(
        &mut self,
        window: PxRect,
        draw: impl FnOnce(&mut DrawLayers) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut inner = DrawLayers::default();
        draw(&mut inner)?;
        let DrawLayers {
            backdrop,
            frame_overlay,
            plain,
            glass,
            content,
            overlay_glass,
            overlay_content,
        } = inner;
        for (outer, items) in [
            (&mut self.backdrop, backdrop),
            (&mut self.frame_overlay, frame_overlay),
            (&mut self.plain, plain),
            (&mut self.glass, glass),
            (&mut self.content, content),
            (&mut self.overlay_glass, overlay_glass),
            (&mut self.overlay_content, overlay_content),
        ] {
            if !items.is_empty() {
                outer.push(DrawItem::Clipped(ClipGroup { rect: window, items }));
            }
        }
        Ok(())
    }

    fn layer_mut(&mut self, layer: Layer) -> &mut Vec<DrawItem> {
        match layer {
            Layer::Backdrop => &mut self.backdrop,
            Layer::FrameOverlay => &mut self.frame_overlay,
            Layer::Plain => &mut self.plain,
            Layer::Glass => &mut self.glass,
            Layer::Content => &mut self.content,
        }
    }

    /// Кладёт собранное `overlay` поверх уже собранного интерфейса: его не-стекло и стекло идут одним слоем
    /// «стекло шторки», содержимое — следующим. Порядок внутри `overlay` сохраняется.
    pub(crate) fn stack_on_top(&mut self, overlay: DrawLayers) {
        let DrawLayers {
            backdrop,
            frame_overlay,
            plain,
            glass,
            content,
            overlay_glass,
            overlay_content,
        } = overlay;
        self.overlay_glass.extend(backdrop);
        self.overlay_glass.extend(frame_overlay);
        self.overlay_glass.extend(plain);
        self.overlay_glass.extend(glass);
        self.overlay_glass.extend(overlay_glass);
        self.overlay_content.extend(content);
        self.overlay_content.extend(overlay_content);
    }

    /// Все элементы в порядке рисования.
    pub(crate) fn into_items(self) -> Vec<DrawItem> {
        let mut items = self.backdrop;
        items.extend(self.frame_overlay);
        items.extend(self.plain);
        items.extend(self.glass);
        items.extend(self.content);
        items.extend(self.overlay_glass);
        items.extend(self.overlay_content);
        items
    }
}

/// Видимые узлы сцены этого кадра с итоговой геометрией. Скрытый узел в индексе отсутствует.
pub(crate) struct VisualIndex {
    visuals: HashMap<NodeId, NodeVisual>,
    px_per_pt: f64,
}

impl VisualIndex {
    pub(crate) fn of(scene: &UiScene<ScreenGesture>) -> Self {
        Self {
            visuals: scene.tree().draw_order().into_iter().map(|v| (v.node, v)).collect(),
            px_per_pt: scene.point_scale().pixels_per_point(),
        }
    }

    /// Размещение в пикселях; `None` — узел скрыт (самим или предком).
    pub(crate) fn placement(&self, node: NodeId) -> Option<NodePlacementPx> {
        self.visuals
            .get(&node)
            .map(|visual| node_placement_px(visual, self.px_per_pt))
    }
}
