//! Меню Controls (iOS 27): шторка проявляется на месте (K7 `PanelMorph::in_place`, ≈ 0,25 с, пункты въезжают
//! снизу на 4,9 pt), сетка 3 × 3 круглых пунктов с заглавными подписями из
//! `menu_tools()`. Варианты инструмента показывает не шторка, а нижняя карточка (`tool_card.rs`, решение 103):
//! пока карточка открыта из меню, шторка остаётся «под ней» открытой (чтобы «‹» вернул в меню), но её стекло и
//! пункты гаснут на долю, которую карточка уже закрыла (`card_cover`, стекло не на стекле).
//!
//! Закрытие — свайп вниз по шторке или тап по видоискателю (его решает редьюсер). Затвор и режимы под
//! шторкой скрывают сами (решение 94, `motion.md` 4.1).

use ui_core::gesture::{GestureEvent, GestureState, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::DrawItem;
use ui_draw::icons::IconId;
use ui_draw::transform::Affine2;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase, PanelSnapshot};
use ui_kit::tokens::{GlassRole, MotionToken};
use ui_kit::view::panel_glass_group;

use super::button::{ButtonEvent, PressButton};
use super::error::{WidgetError, element};
use super::geometry::MAX_MENU_ITEMS;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::sheet_view::{ItemFace, ItemNodes, ItemView, draw_menu_item, draw_sheet_handle};
use super::text_runs::TextRuns;
use super::tool_card_content::flash_icon;
use super::tool_card_content::tool_title;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::settings::{AspectRatio, GridSetting, LevelSetting, RawCapture, TimerSetting};
use crate::state::PanelState;
use crate::tools::ToolId;

struct ItemSlot {
    button: PressButton,
    content: NodeId,
    tool: Option<ToolId>,
}

pub(crate) struct ControlsDrawerWidget {
    sheet_node: NodeId,
    sheet_morph: PanelMorph,
    sheet_open_wanted: bool,
    items: Vec<ItemSlot>,
}

/// Пружина открытия и закрытия шторки: без перелёта, ≈ 0,3 с (`[measured: C t=22.88–23.15]` ≈ 0,25 с).
pub(crate) const SHEET_MOTION: MotionToken = MotionToken::CardMorph;

/// Включён ли инструмент (жёлтый круг): отличается от значения по умолчанию.
fn tool_is_active(ctx: &Ctx<'_>, tool: ToolId) -> bool {
    let settings = ctx.state.settings();
    match tool {
        ToolId::Flash => settings.flash.is_highlighted(),
        ToolId::Raw => settings.raw == RawCapture::On,
        ToolId::Timer => settings.timer != TimerSetting::Off,
        ToolId::Exposure => ctx.state.exposure().steps() != 0,
        ToolId::Aspect => settings.aspect != AspectRatio::FourByThree,
        ToolId::Grid => settings.grid == GridSetting::On,
        ToolId::Level => settings.level == LevelSetting::On,
        // Белого в шторке Controls нет (значок только верхнего ряда); ветка нужна полноте match.
        ToolId::White | ToolId::Live | ToolId::Styles | ToolId::Night => false,
    }
}

impl ControlsDrawerWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let sheet_rect = ctx.geometry.sheet();
        let sheet_node = kit.add_node(parent, "controls_sheet", sheet_rect, ctx.geometry.screen())?;
        kit.add_recognizer(
            sheet_node,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Down,
                ..PanSpec::default()
            }),
            ScreenGesture::DrawerClosePan,
        )?;
        let menu_tools = ctx.state.menu_tools();
        if menu_tools.len() > MAX_MENU_ITEMS {
            return Err(WidgetError::Inconsistent(format!(
                "в меню {} пунктов, в сетке {MAX_MENU_ITEMS}",
                menu_tools.len()
            )));
        }
        let mut items = Vec::with_capacity(MAX_MENU_ITEMS);
        for slot in 0..MAX_MENU_ITEMS {
            let rect = ctx.geometry.menu_item(slot);
            let node = kit.add_node(sheet_node, "controls_item", rect, sheet_rect)?;
            let button = PressButton::attach(kit, ButtonId::MenuItem(slot as u8), node, rect, ctx)?;
            let content = kit.add_rotating_content(node, "controls_item_content")?;
            let tool = menu_tools.get(slot).copied();
            if tool.is_none() {
                kit.set_shown(node, false)?;
            }
            items.push(ItemSlot { button, content, tool });
        }
        kit.set_interactive(sheet_node, false)?;

        // iOS 27: шторка не растёт из кнопки, а проявляется на месте; пункты въезжают снизу.
        let sheet_config = &ctx.geometry.config().sheet;
        let spec = PanelMorphSpec::in_place(sheet_rect, sheet_config.corner_radius_pt, SHEET_MOTION)
            .with_content_slide_pt(sheet_config.open_content_slide_pt);
        let sheet_morph =
            PanelMorph::new(spec, &ctx.environment(sheet_rect.size.height), ctx.now).map_err(element("шторка меню"))?;
        Ok(Self {
            sheet_node,
            sheet_morph,
            sheet_open_wanted: false,
            items,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::DrawerClosePan => {
                // Закрывает завершённый жест; отменённый (разрыв потока) — без действия.
                if event.state == GestureState::Ended {
                    out.actions.push(ScreenAction::ControlsMenuCloseRequested);
                }
            }
            ScreenGesture::Press(ButtonId::MenuItem(slot)) => {
                let item = self
                    .items
                    .get_mut(usize::from(slot))
                    .ok_or_else(|| WidgetError::Inconsistent(format!("пункта меню {slot} нет")))?;
                if item.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated
                    && let Some(tool) = item.tool
                {
                    out.actions.push(ScreenAction::ControlsItemSelected(tool));
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Прозрачность кнопки меню: шторка проявляется поверх неё на месте, и кнопка гаснет вместе с появлением
    /// стекла шторки (стекло не на стекле). У морфа «на месте» своего `source_opacity` нет (источника нет),
    /// поэтому берём долю материализации стекла.
    pub(crate) fn source_opacity(&self, now: ui_core::time::MonotonicNanos) -> f32 {
        1.0 - self.sheet_morph.snapshot(now).effect.clamp(0.0, 1.0)
    }

    /// Шторка открыта, пока открыто меню или карточка, пришедшая из меню (под ней шторка ждёт «‹»).
    /// Касания шторка ловит только на уровне меню: поверх карточки она не должна перехватывать тапы.
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let menu_level = ctx.state.panel() == PanelState::ControlsMenu;
        let sheet_open = menu_level || ctx.state.card_returns_to_menu();
        if sheet_open != self.sheet_open_wanted {
            self.sheet_open_wanted = sheet_open;
            if sheet_open {
                self.sheet_morph.open(ctx.now).map_err(element("шторка меню"))?;
            } else {
                self.sheet_morph.close(ctx.now).map_err(element("шторка меню"))?;
            }
        }
        kit.set_interactive(self.sheet_node, menu_level)?;
        // Состав меню зависит от возможностей камеры (вспышка появляется и пропадает по фактам): пункты идут
        // подряд, пустые слоты скрыты и касаний не принимают.
        let menu_tools = ctx.state.menu_tools();
        for (slot, item) in self.items.iter_mut().enumerate() {
            let tool = menu_tools.get(slot).copied();
            if item.tool.is_some() != tool.is_some() {
                kit.set_shown(item.button.node(), tool.is_some())?;
            }
            item.tool = tool;
            kit.set_interactive(item.button.node(), menu_level && tool.is_some())?;
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>) {
        self.sheet_morph.advance(ctx.now);
        let _sheet_events = self.sheet_morph.drain_events();
    }

    /// `card_cover` — доля 0..1, на которую нижняя карточка уже закрыла шторку: стекло и пункты гаснут.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        card_cover: f32,
    ) -> Result<(), WidgetError> {
        let sheet = self.sheet_morph.snapshot(ctx.now);
        if sheet.phase == PanelPhase::Closed {
            return Ok(());
        }
        let uncovered = (1.0 - card_cover).clamp(0.0, 1.0);
        // Стекло не на стекле: пока карточка закрывает шторку, её собственное стекло гаснет.
        let sheet_glass = PanelSnapshot {
            effect: sheet.effect * uncovered,
            ..sheet
        };
        layers.push(
            Layer::Glass,
            DrawItem::Glass(panel_glass_group(
                &sheet_glass,
                ctx.tokens.glass.style_for(GlassRole::Panel),
                ctx.px,
            )?),
        );
        let content_alpha = sheet.content_opacity * uncovered;
        draw_sheet_handle(
            ctx,
            layers,
            ctx.geometry.sheet_handle(),
            (Affine2::IDENTITY, content_alpha),
        );
        self.draw_items(
            ctx,
            visuals,
            layers,
            texts,
            content_alpha,
            ctx.px.px(sheet.content_offset_pt),
        )
    }

    fn draw_items(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        alpha: f32,
        slide_px: f32,
    ) -> Result<(), WidgetError> {
        if alpha <= 0.0 {
            return Ok(());
        }
        for item in &self.items {
            let (Some(tool), Some(button), Some(content)) = (
                item.tool,
                visuals.placement(item.button.node()),
                visuals.placement(item.content),
            ) else {
                continue;
            };
            let face = match tool_icon(ctx, tool) {
                Some(icon) => ItemFace::Icon(icon),
                None => ItemFace::Text(tool_title(tool)),
            };
            let view = ItemView {
                face,
                caption: tool_title(tool),
                highlighted: tool_is_active(ctx, tool),
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

/// Значок пункта; `None` — у инструмента значка нет, в круге стоит подпись.
fn tool_icon(ctx: &Ctx<'_>, tool: ToolId) -> Option<IconId> {
    let settings = ctx.state.settings();
    match tool {
        ToolId::Flash => Some(flash_icon(settings.flash)),
        ToolId::Timer => Some(IconId::Timer),
        ToolId::Exposure => Some(IconId::Exposure),
        ToolId::Aspect => Some(IconId::AspectRatio),
        ToolId::Grid => Some(IconId::Grid),
        ToolId::Level => Some(IconId::Level),
        // RAW у iOS остаётся подписью внутри круга, не значком. Для STYLES подходящего значка в наборе нет
        // (палитра, фильтр): подпись в круге, а не новый рисунок.
        ToolId::White | ToolId::Raw | ToolId::Live | ToolId::Styles | ToolId::Night => None,
    }
}
