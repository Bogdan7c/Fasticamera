//! Шторка корня меню раздела калибровки: два круглых пункта (PRESETS, SCOPE AXES) на стеклянной панели.
//!
//! Тот же вид, что у шторки Controls экрана PHOTO (решения 94, 199): панель проявляется на месте (`PanelMorph`),
//! пункты рисует общий `sheet_view::draw_menu_item`, числа берутся из тех же `widgets.sheet`. Отличия данные, не
//! код: пунктов два, они стоят одним рядом по центру, высота шторки считается по рядам (полная шторка 464 pt в
//! альбомный глазной прямоугольник 440 pt не помещается), а всё лежит под повёрнутым корнем раздела.

use crate::widget::SECTION_PANEL_MATERIAL;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase, PanelSnapshot};
use ui_kit::view::panel_material_hosted;

use crate::action::ScreenAction;
use crate::calibration::{CalibrationMenu, CalibrationMenuItem};
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::calibration::host::RootHost;
use crate::widget::calibration::space::{EyeAxis, SectionSpace};
use crate::widget::controls_drawer::SHEET_MOTION;
use crate::widget::error::{WidgetError, element};
use crate::widget::geometry::ScreenGeometry;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::sheet_view::{ItemFace, ItemNodes, ItemView, draw_menu_item, draw_sheet_handle};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Как выглядит пункт корня меню: короткая подпись в круге и заглавная подпись под ним. Значка в наборе нет
/// (палитра, фильтр), нового не рисуем — как у RAW и STYLES в PHOTO. `[invented]`.
fn item_look(item: CalibrationMenuItem) -> (&'static str, &'static str) {
    match item {
        CalibrationMenuItem::Presets => ("LOOK", "PRESETS"),
        CalibrationMenuItem::ScopeAxes => ("XY", "SCOPE AXES"),
    }
}

struct SheetItem {
    item: CalibrationMenuItem,
    button: PressButton,
    content: NodeId,
}

pub(super) struct MenuSheet {
    node: NodeId,
    rect: Rect,
    /// «Ручка» шторки (только рисунок), в системе корня.
    handle: Rect,
    morph: PanelMorph,
    open_wanted: bool,
    items: Vec<SheetItem>,
}

impl MenuSheet {
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (eye, space): (&ScreenGeometry, &SectionSpace),
        (root, root_frame): (NodeId, Rect),
    ) -> Result<Self, WidgetError> {
        let widgets = &ctx.config.calibration.widgets;
        let rect = eye.sheet_for_rows(1, widgets.menu_sheet_bottom_padding_pt);
        let node = kit.add_node(root, "calib_menu_sheet", rect, root_frame)?;
        // Свайп вниз глазами закрывает меню (решает `SectionMenu` по смещению); распознаватель — на шторке.
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Vertical),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationMenuClosePan,
        )?;
        let mut items = Vec::with_capacity(CalibrationMenuItem::ALL.len());
        for (slot, item) in CalibrationMenuItem::ALL.into_iter().enumerate() {
            let item_rect = eye.menu_item_centered_in(rect, slot, CalibrationMenuItem::ALL.len());
            let item_node = kit.add_node(node, "calib_menu_item", item_rect, rect)?;
            let slot_id = u8::try_from(slot)
                .map_err(|_| WidgetError::Inconsistent(format!("пункт меню раздела {slot} не влезает в u8")))?;
            let button = PressButton::attach(kit, ButtonId::CalibrationMenuItem(slot_id), item_node, item_rect, ctx)?;
            let content = kit.add_fixed_content(item_node, "calib_menu_item_content")?;
            kit.set_interactive(item_node, false)?;
            items.push(SheetItem { item, button, content });
        }
        kit.set_interactive(node, false)?;
        let sheet_config = &ctx.geometry.config().sheet;
        let spec = PanelMorphSpec::in_place(rect, sheet_config.corner_radius_pt, SHEET_MOTION)
            .with_content_slide_pt(sheet_config.open_content_slide_pt);
        let morph = PanelMorph::new(spec, &ctx.environment(rect.size.height), ctx.now)
            .map_err(element("шторка меню раздела"))?;
        Ok(Self {
            node,
            rect,
            handle: eye.sheet_handle_in(rect),
            morph,
            open_wanted: false,
            items,
        })
    }

    /// Рамка шторки в системе корня: из неё вырастает карточка.
    pub(super) fn rect(&self) -> Rect {
        self.rect
    }

    /// Шторка открыта, пока открыто любое меню раздела; касания ловит только на уровне корня (под карточкой ждёт «‹»).
    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let menu = ctx.state.calibration().menu();
        let sheet_open = menu != CalibrationMenu::Closed;
        let root_level = menu == CalibrationMenu::Root;
        if sheet_open != self.open_wanted {
            self.open_wanted = sheet_open;
            if sheet_open {
                self.morph.open(ctx.now).map_err(element("шторка меню раздела"))?;
            } else {
                self.morph.close(ctx.now).map_err(element("шторка меню раздела"))?;
            }
        }
        kit.set_interactive(self.node, root_level)?;
        for item in &self.items {
            kit.set_interactive(item.button.node(), root_level)?;
        }
        Ok(())
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>) {
        self.morph.advance(ctx.now);
        let _events = self.morph.drain_events();
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let ScreenGesture::Press(ButtonId::CalibrationMenuItem(slot)) = event.intent else {
            return Ok(false);
        };
        let item = self
            .items
            .get_mut(usize::from(slot))
            .ok_or_else(|| WidgetError::Inconsistent(format!("пункта меню раздела {slot} нет")))?;
        let data = kit.gesture_data_in_node(item.button.node(), event.data);
        if item.button.handle(event.state, data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::CalibrationMenuItemTapped(item.item));
        }
        Ok(true)
    }

    /// `card_cover` — доля 0..1, на которую карточка уже закрыла шторку: стекло и пункты гаснут (стекло не на стекле).
    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        (visuals, host): (&VisualIndex, &RootHost),
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        card_cover: f32,
    ) -> Result<(), WidgetError> {
        let sheet = self.morph.snapshot(ctx.now);
        if sheet.phase == PanelPhase::Closed {
            return Ok(());
        }
        let uncovered = (1.0 - card_cover).clamp(0.0, 1.0);
        let glass = PanelSnapshot {
            effect: sheet.effect * uncovered,
            ..sheet
        };
        layers.push(
            Layer::Glass,
            panel_material_hosted(&glass, ctx.tokens, SECTION_PANEL_MATERIAL, ctx.px, host.transform())?,
        );
        let alpha = sheet.content_opacity * uncovered;
        draw_sheet_handle(ctx, layers, self.handle, (host.transform(), alpha));
        if alpha <= 0.0 {
            return Ok(());
        }
        let slide_px = ctx.px.px(sheet.content_offset_pt);
        for item in &self.items {
            let (Some(button), Some(content)) =
                (visuals.placement(item.button.node()), visuals.placement(item.content))
            else {
                continue;
            };
            let (face, caption) = item_look(item.item);
            let view = ItemView {
                face: ItemFace::Text(face),
                caption,
                highlighted: false,
            };
            draw_menu_item(
                ctx,
                layers,
                texts,
                ItemNodes { button, content },
                view,
                (alpha, slide_px),
            )?;
        }
        Ok(())
    }
}
