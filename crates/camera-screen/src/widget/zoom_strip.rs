//! Лента кнопок зума (K2 × N): подписи берутся из каталога камер, выбранная всегда стоит на одном месте, лента
//! едет под ней (`layout.md` 2.2, `[measured: R00 t=88.5–100]`).
//!
//! # Устройство
//! * `container` — зона ленты: на ней долгий тап и пан, которые открывают колесо (`behavior.md` 2.3);
//! * `tape` — узел-«лента»: его визуальный сдвиг `TranslationX` анимируется при смене выбранной кнопки, кнопки —
//!   его дети. Раскладка кнопок всегда «финальная» (выбранная на `selected_x_pt`), поэтому рамки трекеров
//!   нажатия верны в покое; сдвиг ленты — только вид, как подъём затвора;
//! * кнопок ровно `zoom.strip.max_buttons` (узлы создаются при сборке сцены, а каталог приходит позже — как
//!   слоты значков над кадром); лишние спрятаны, а упоров больше слотов — явная ошибка сборки, не обрезка.
//!
//! Ленты нет на фронтальной камере и пока открыто колесо, меню или карточка (решения 112, 115).

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, LongPressSpec, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::MotionPreference;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::{MotionToken, TextRoleKind};
use ui_kit::view::{centered_text, opacity_of_placement};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::state::PanelState;
use crate::zoom_labels::{selected_label, unselected_label};

pub(crate) struct ZoomStripWidget {
    container: NodeId,
    tape: NodeId,
    tape_rect: Rect,
    buttons: Vec<PressButton>,
    /// Что раскладка кнопок показывает сейчас: (выбранная, сколько кнопок). `None` — ещё не раскладывали.
    laid_out: Option<(usize, usize)>,
    /// (зона ловит жесты, лента кнопок видна), как настроено сейчас.
    applied: Option<(bool, bool)>,
}

impl ZoomStripWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let container_rect = ctx.geometry.zoom_strip();
        let container = kit.add_node(parent, "zoom_strip", container_rect, ctx.geometry.screen())?;
        // Удержание и перетаскивание ленты открывают колесо (`behavior.md` 2.3): оба пути ведут один и тот же палец.
        kit.add_recognizer(
            container,
            RecognizerSpec::LongPress(LongPressSpec::default()),
            ScreenGesture::ZoomStripLongPress,
        )?;
        kit.add_recognizer(
            container,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Horizontal,
                ..PanSpec::default()
            }),
            ScreenGesture::ZoomStripPan,
        )?;
        let tape = kit.add_node(container, "zoom_tape", container_rect, container_rect)?;
        let slots = ctx.geometry.zoom_config().strip.max_buttons;
        let mut buttons = Vec::with_capacity(usize::from(slots));
        for slot in 0..slots {
            let frame = ctx.geometry.zoom_button(usize::from(slot), 0);
            let node = kit.add_node(tape, "zoom_button", frame, container_rect)?;
            buttons.push(PressButton::attach(kit, ButtonId::ZoomStop(slot), node, frame, ctx)?);
        }
        // До первых фактов ленты нет.
        kit.set_interactive(container, false)?;
        kit.set_shown(tape, false)?;
        Ok(Self {
            container,
            tape,
            tape_rect: container_rect,
            buttons,
            laid_out: None,
            applied: Some((false, false)),
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let ScreenGesture::Press(ButtonId::ZoomStop(slot)) = event.intent else {
            return Ok(false);
        };
        let Some(button) = self.buttons.get_mut(usize::from(slot)) else {
            return Ok(false);
        };
        if button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::ZoomStopTapped {
                stop: usize::from(slot),
                at: event.time,
            });
        }
        Ok(true)
    }

    /// Зона ленты живёт, пока есть шкала задней камеры и нет панелей: тогда долгий тап и пан по ней ведут колесо.
    fn zone_active(ctx: &Ctx<'_>) -> bool {
        use camera_contract::ids::LensFacing;
        ctx.state.zoom_group().is_some()
            && ctx.state.zoom().facing() == LensFacing::Back
            && ctx.state.panel() == PanelState::Closed
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        // Пока колесо открыто, кнопки спрятаны, а зона остаётся: палец, открывший колесо, продолжает его вести
        // (отключённый узел сбросил бы идущий жест).
        let active = Self::zone_active(ctx);
        let tape_visible = active && !ctx.state.zoom().is_wheel_open();
        if self.applied != Some((active, tape_visible)) {
            self.applied = Some((active, tape_visible));
            // Лента пропадает и появляется за кадр (`layout.md`: «зум исчезает за 1 кадр», R00 t=11.1 и 19.45).
            kit.set_interactive(self.container, active)?;
            kit.set_shown(self.tape, tape_visible)?;
        }
        let Some(group) = ctx.state.zoom_group() else {
            return Ok(());
        };
        let count = group.stops.len();
        if count > self.buttons.len() {
            return Err(WidgetError::Inconsistent(format!(
                "в каталоге {count} упоров зума, а в ленте {} кнопок (zoom.strip.max_buttons)",
                self.buttons.len()
            )));
        }
        let selected = ctx.state.zoom().selected_stop_index(group);
        if self.laid_out == Some((selected, count)) {
            return Ok(());
        }
        let previous_selected = self.laid_out.map(|(selected, _)| selected);
        for (slot, button) in self.buttons.iter_mut().enumerate() {
            if slot < count {
                let frame = ctx.geometry.zoom_button(slot, selected);
                kit.set_shown(button.node(), true)?;
                kit.move_node(button.node(), frame, self.tape_rect)?;
                button.set_frame(frame)?;
            } else {
                kit.set_shown(button.node(), false)?;
            }
        }
        self.laid_out = Some((selected, count));
        if let Some(previous) = previous_selected.filter(|previous| *previous != selected) {
            self.slide_from(kit, ctx, previous, selected)?;
        }
        Ok(())
    }

    /// Выбранная кнопка сменилась: раскладка уже новая, а лента визуально стоит на прежнем месте и едет к нулю.
    /// Сдвиг складывается с идущим (повторный тап во время сдвига не даёт скачка).
    fn slide_from(&self, kit: &mut SceneKit, ctx: &Ctx<'_>, from: usize, to: usize) -> Result<(), WidgetError> {
        let pitch = ctx.geometry.zoom_config().strip.pitch_pt;
        let start = kit.horizontal_offset_pt(self.tape)? + pitch * (to as f64 - from as f64);
        if ctx.motion == MotionPreference::Reduced {
            // Reduce Motion: без перемещений, лента сразу на месте.
            return kit.jump_horizontally_to(self.tape, 0.0);
        }
        kit.jump_horizontally_to(self.tape, start)?;
        // ≈ 0,3 с без перелёта: измеренный сдвиг ленты 0,27–0,30 с (`motion.md` M:124).
        kit.slide_horizontally_to(self.tape, 0.0, ctx.spring(MotionToken::CardMorph)?, ctx.now)
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(group) = ctx.state.zoom_group() else {
            return Ok(());
        };
        let selected = ctx.state.zoom().selected_stop_index(group);
        let strip = &ctx.geometry.zoom_config().strip;
        let colors = &ctx.tokens.colors;
        for (slot, stop) in group.stops.iter().enumerate() {
            let Some(button) = self.buttons.get(slot) else {
                break;
            };
            let Some(placement) = visuals.placement(button.node()) else {
                continue;
            };
            if placement.opacity <= 0.0 {
                continue;
            }
            let node_opacity = opacity_of_placement(&placement, "zoom_button")?;
            let is_selected = slot == selected;
            let (text, color) = if is_selected {
                (selected_label(ctx.state.zoom().ratio()), colors.zoom_selected)
            } else {
                (unselected_label(stop.ratio), colors.text_primary)
            };
            if is_selected {
                // Тёмный полупрозрачный круг под выбранной подписью.
                let circle_opacity = scaled(node_opacity, strip.selected_circle_alpha)?;
                layers.push(
                    Layer::Plain,
                    DrawItem::Shape(ShapeItem {
                        geometry: ShapeGeometry::Circle {
                            diameter: ctx.px.px(strip.selected_circle_diameter_pt),
                        },
                        transform: placement.transform,
                        fill: DisplayColor::BLACK,
                        stroke: None,
                        shadow: None,
                        opacity: circle_opacity,
                    }),
                );
            }
            let run = texts.run(ctx, &text, TextRoleKind::ZoomLabel)?;
            layers.push(
                Layer::Content,
                centered_text(run, placement.transform, color, node_opacity),
            );
        }
        Ok(())
    }
}

fn scaled(opacity: Opacity, factor: f32) -> Result<Opacity, WidgetError> {
    Opacity::new(opacity.get() * factor)
        .map_err(|_| WidgetError::Inconsistent(format!("прозрачность {} × {factor} вне 0..1", opacity.get())))
}
