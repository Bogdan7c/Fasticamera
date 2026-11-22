//! Значки над кадром (`visible_strip_tools()`): вспышка, RAW, таймер (только когда включён), EV.
//!
//! Ряд — данные (М3): слоты узлов создаются заранее, а какой инструмент в каком слоте, решает состояние на
//! каждом кадре. Значки без стекла; активное состояние — жёлтым. EV — не значок, а две строки текста («0.0» над «EV»).

use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{centered_icon, centered_text, opacity_of_placement};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::exposure_link::format_ev;
use super::geometry::MAX_STRIP_ICONS;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::tool_card_content::flash_icon;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::settings::RawCapture;
use crate::tools::ToolId;
use ui_core::geometry::Rect;

struct Slot {
    button: PressButton,
    content: NodeId,
    /// Какой инструмент стоит в слоте сейчас (`None` — слот пуст и спрятан).
    tool: Option<ToolId>,
    placed: Option<Rect>,
}

pub(crate) struct TopBarWidget {
    slots: Vec<Slot>,
}

impl TopBarWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let mut slots = Vec::with_capacity(MAX_STRIP_ICONS);
        for index in 0..MAX_STRIP_ICONS {
            let id = ButtonId::StripSlot(index as u8);
            let frame = ctx.geometry.strip_icon(index, MAX_STRIP_ICONS);
            let node = kit.add_node(parent, "strip_icon", frame, ctx.geometry.screen())?;
            let button = PressButton::attach(kit, id, node, frame, ctx)?;
            let content = kit.add_rotating_content(node, "strip_icon_content")?;
            kit.set_shown(node, false)?;
            slots.push(Slot {
                button,
                content,
                tool: None,
                placed: None,
            });
        }
        Ok(Self { slots })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let ScreenGesture::Press(ButtonId::StripSlot(index)) = event.intent else {
            return Ok(false);
        };
        let Some(slot) = self.slots.get_mut(usize::from(index)) else {
            return Err(WidgetError::Inconsistent(format!("слота значков {index} нет")));
        };
        if slot.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated
            && let Some(tool) = slot.tool
        {
            out.actions.push(ScreenAction::StripToolTapped(tool));
        }
        Ok(true)
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let tools = ctx.state.visible_strip_tools();
        if tools.len() > self.slots.len() {
            return Err(WidgetError::Inconsistent(format!(
                "значков над кадром {}, слотов {}",
                tools.len(),
                self.slots.len()
            )));
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let tool = tools.get(index).copied();
            if tool.is_some() {
                let rect = ctx.geometry.strip_icon(index, tools.len());
                if slot.placed != Some(rect) {
                    kit.move_node(slot.button.node(), rect, ctx.geometry.screen())?;
                    slot.button.set_frame(rect)?;
                    slot.placed = Some(rect);
                }
            }
            if slot.tool.is_some() != tool.is_some() {
                kit.set_shown(slot.button.node(), tool.is_some())?;
            }
            slot.tool = tool;
        }
        Ok(())
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let strip = &ctx.geometry.config().strip;
        for slot in &self.slots {
            let (Some(tool), Some(placement)) = (slot.tool, visuals.placement(slot.content)) else {
                continue;
            };
            let colors = &ctx.tokens.colors;
            let opacity = opacity_of_placement(&placement, "strip_icon")?;
            let host = placement.transform;
            let icon = |icon: IconId, color: DisplayColor| {
                centered_icon(icon, strip.icon_size_pt, ctx.px, host, color, opacity)
            };
            let below = Affine2::translate(0.0, ctx.px.px(strip.value_below_icon_pt)).then(host);
            let settings = ctx.state.settings();
            match tool {
                ToolId::Flash => {
                    // Жёлтым — только On; Auto — белый значок с «A» (см. `FlashMode::is_highlighted`).
                    let color = if settings.flash.is_highlighted() {
                        colors.focus_yellow
                    } else {
                        colors.text_primary
                    };
                    layers.push(Layer::Content, icon(flash_icon(settings.flash), color));
                }
                ToolId::Raw => {
                    let on = settings.raw == RawCapture::On;
                    let color = if on { colors.focus_yellow } else { colors.text_primary };
                    let run = texts.run(ctx, "RAW", TextRoleKind::Caption)?;
                    layers.push(Layer::Content, centered_text(run, host, color, opacity));
                    if !on {
                        // Зачёркнутый RAW: линия поверх подписи — фигура, а не отдельная иконка.
                        layers.push(
                            Layer::Content,
                            DrawItem::Shape(ShapeItem {
                                geometry: ShapeGeometry::RoundedRect {
                                    size: PxSize::new(
                                        run.advance_px * strip.raw_strike_length_scale,
                                        ctx.px.px(ctx.geometry.config().overlay.level_thickness_pt),
                                    ),
                                    corner: CornerSpec::circular(0.0),
                                },
                                transform: Affine2::rotate(strip.raw_strike_angle_rad).then(host),
                                fill: color,
                                stroke: None,
                                shadow: None,
                                opacity,
                            }),
                        );
                    }
                }
                ToolId::Timer => {
                    layers.push(Layer::Content, icon(IconId::Timer, colors.focus_yellow));
                    if let Some(seconds) = ctx.config.timer.seconds_for(settings.timer) {
                        let run = texts.run(ctx, &seconds.to_string(), TextRoleKind::Caption)?;
                        layers.push(Layer::Content, centered_text(run, below, colors.focus_yellow, opacity));
                    }
                }
                ToolId::Exposure => {
                    let steps = ctx.state.exposure().steps();
                    let color = if steps == 0 {
                        colors.text_primary
                    } else {
                        colors.focus_yellow
                    };
                    // iOS 27: вместо значка солнца — две строки текста, число над «EV»; жёлтые при EV ≠ 0.
                    let ev = ctx.state.exposure().bias(&ctx.config.exposure).ev();
                    let line_at = |offset_pt: f64| Affine2::translate(0.0, ctx.px.px(offset_pt)).then(host);
                    let value = texts.run(ctx, &format_ev(ev), TextRoleKind::StripValue)?;
                    layers.push(
                        Layer::Content,
                        centered_text(value, line_at(strip.ev_value_offset_pt), color, opacity),
                    );
                    let unit = texts.run(ctx, "EV", TextRoleKind::StripUnit)?;
                    layers.push(
                        Layer::Content,
                        centered_text(unit, line_at(strip.ev_unit_offset_pt), color, opacity),
                    );
                }
                // В ряд над кадром другие инструменты не пускает `ToolSet::new`.
                ToolId::Aspect | ToolId::Grid | ToolId::Level | ToolId::Live | ToolId::Styles | ToolId::Night => {
                    return Err(WidgetError::Inconsistent(format!(
                        "{tool:?} не может быть значком над кадром"
                    )));
                }
            }
        }
        Ok(())
    }
}
