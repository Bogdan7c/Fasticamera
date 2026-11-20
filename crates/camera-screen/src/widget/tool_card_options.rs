//! Ряд вариантов нижней карточки: значки или короткие подписи, выбранный — жёлтым (решение 103).
//!
//! Ряд — данные: слотов столько, сколько вариантов у самого длинного инструмента (`MAX_SUBMENU_OPTIONS`),
//! а какой вариант в каком слоте и сколько слотов занято, решает [`OptionRow::bind`] при открытии карточки
//! (слоты переезжают, ряд остаётся центрированным по x). Выбранный вариант ряд не хранит: его даёт состояние
//! экрана на каждом кадре, поэтому подсветка всегда показывает настройку, а не «последний тап».
//!
//! # Жизненный цикл при закрытии
//! Закрытие карточки идёт ≈ 0,3 с, и варианты должны гаснуть вместе с ней, а не пропадать в первый кадр.
//! Поэтому снятие касаний ([`OptionRow::stop_touches`]) и забвение вариантов ([`OptionRow::unbind`]) —
//! разные шаги: первое делается сразу, второе — когда морф карточки дошёл до `Closed`.

use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_draw::color::Opacity;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{centered_icon, centered_text};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::geometry::MAX_SUBMENU_OPTIONS;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::tool_card_content::{OptionEntry, OptionFace};
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::tools::{ToolId, ToolOption};

/// Слот ряда: кнопка и узел значка (поворачивается по ориентации телефона).
struct OptionSlot {
    button: PressButton,
    content: NodeId,
}

/// Ряд вариантов карточки.
pub(crate) struct OptionRow {
    slots: Vec<OptionSlot>,
    /// Инструмент, чьи варианты лежат в слотах; `None` — ряд не привязан.
    bound_tool: Option<ToolId>,
    /// Что лежит в слотах сейчас, слева направо; пусто, пока ряд не привязан.
    entries: Vec<OptionEntry>,
    /// Ловят ли занятые слоты касания. Выключается при закрытии, а варианты остаются нарисованными.
    touchable: bool,
}

impl OptionRow {
    /// Слоты создаются спрятанными от касаний; рамки временные (первый слот из четырёх), `bind` их переставит.
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        card_node: NodeId,
        card_rect: Rect,
    ) -> Result<Self, WidgetError> {
        let mut slots = Vec::with_capacity(MAX_SUBMENU_OPTIONS);
        for index in 0..MAX_SUBMENU_OPTIONS {
            let rect = ctx.geometry.card_option_slot(index, MAX_SUBMENU_OPTIONS);
            let node = kit.add_node(card_node, "card_option", rect, card_rect)?;
            let button = PressButton::attach(kit, ButtonId::CardOption(index as u8), node, rect, ctx)?;
            let content = kit.add_rotating_content(node, "card_option_icon")?;
            kit.set_interactive(node, false)?;
            slots.push(OptionSlot { button, content });
        }
        Ok(Self {
            slots,
            bound_tool: None,
            entries: Vec::new(),
            touchable: false,
        })
    }

    /// Привязывает ряд к вариантам инструмента: переставляет и включает занятые слоты, остальные выключает.
    pub(crate) fn bind(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        card_rect: Rect,
        tool: ToolId,
        entries: Vec<OptionEntry>,
    ) -> Result<(), WidgetError> {
        if entries.len() > self.slots.len() {
            return Err(WidgetError::Inconsistent(format!(
                "вариантов {}, слотов в ряду карточки {}",
                entries.len(),
                self.slots.len()
            )));
        }
        let count = entries.len();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let occupied = index < count;
            if occupied {
                let rect = ctx.geometry.card_option_slot(index, count);
                kit.move_node(slot.button.node(), rect, card_rect)?;
                slot.button.set_frame(rect)?;
            }
            kit.set_interactive(slot.button.node(), occupied)?;
        }
        self.bound_tool = Some(tool);
        self.entries = entries;
        self.touchable = true;
        Ok(())
    }

    /// Ряд сейчас привязан к вариантам именно этого инструмента.
    pub(crate) fn is_bound_to(&self, tool: ToolId) -> bool {
        self.bound_tool == Some(tool)
    }

    /// Привязан ли ряд хоть к какому-то инструменту.
    pub(crate) fn is_bound(&self) -> bool {
        self.bound_tool.is_some()
    }

    /// Ловят ли слоты касания.
    pub(crate) fn is_touchable(&self) -> bool {
        self.touchable
    }

    /// Слоты перестают ловить касания, а варианты остаются на месте: они ещё рисуются, пока карточка гаснет.
    pub(crate) fn stop_touches(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        if !self.touchable {
            return Ok(());
        }
        for slot in &self.slots {
            kit.set_interactive(slot.button.node(), false)?;
        }
        self.touchable = false;
        Ok(())
    }

    /// Ряд забывает варианты (и сперва снимает касания): карточка закрылась целиком или сменилась на карточку
    /// без ряда (Exposure).
    pub(crate) fn unbind(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        self.stop_touches(kit)?;
        self.forget_entries();
        Ok(())
    }

    /// Забыть варианты, не трогая сцену: касания к этому моменту уже сняты ([`Self::stop_touches`]).
    pub(crate) fn forget_entries(&mut self) {
        self.bound_tool = None;
        self.entries.clear();
    }

    /// Тап по слоту → выбор варианта. Карточка при этом остаётся открытой (решает редьюсер).
    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let ScreenGesture::Press(ButtonId::CardOption(index)) = event.intent else {
            return Ok(false);
        };
        let index = usize::from(index);
        let slot = self
            .slots
            .get_mut(index)
            .ok_or_else(|| WidgetError::Inconsistent(format!("слота варианта {index} нет")))?;
        if slot.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated
            && let Some(entry) = self.entries.get(index)
        {
            out.actions.push(ScreenAction::SubmenuOptionSelected(entry.option));
        }
        Ok(true)
    }

    /// Рисует ряд. `selected` — выбранный сейчас вариант из состояния; `alpha` и `slide_px` — прозрачность и
    /// сдвиг содержимого карточки из морфа.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        selected: Option<ToolOption>,
        (alpha, slide_px): (f32, f32),
    ) -> Result<(), WidgetError> {
        let colors = &ctx.tokens.colors;
        let slide = Affine2::translate(0.0, slide_px);
        for (slot, entry) in self.slots.iter().zip(&self.entries) {
            let (Some(button), Some(content)) =
                (visuals.placement(slot.button.node()), visuals.placement(slot.content))
            else {
                continue;
            };
            let opacity = Opacity::new((button.opacity * alpha).clamp(0.0, 1.0))
                .map_err(|_| WidgetError::Inconsistent("прозрачность варианта вне 0..1".to_owned()))?;
            let color = if selected == Some(entry.option) {
                colors.focus_yellow
            } else {
                colors.text_primary
            };
            match &entry.face {
                OptionFace::Icon(icon) => layers.push(
                    Layer::Content,
                    centered_icon(
                        *icon,
                        ctx.geometry.config().card.option_icon_size_pt,
                        ctx.px,
                        slide.then(content.transform),
                        color,
                        opacity,
                    ),
                ),
                OptionFace::Label(text) => {
                    // Цифры таймера и «4:3» — табличные, чтобы ряд не «прыгал» при смене выбранного.
                    let run = texts.run(ctx, text, TextRoleKind::CardOption)?;
                    layers.push(
                        Layer::Content,
                        centered_text(run, slide.then(button.transform), color, opacity),
                    );
                }
            }
        }
        Ok(())
    }
}
