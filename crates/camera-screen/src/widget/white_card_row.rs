//! Ряд «Phone» + кнопки света карточки белого (WB28, WB30, WB32, WB33).
//!
//! Все кнопки видны без прокрутки (число проверяет конфиг). Кнопка — значок над подписью; активная — жёлтая.
//! «Phone» ещё несёт строку оценки «≈ 3100» на постоянном месте под подписью (место зарезервировано, раскладка
//! не прыгает, пока оценки нет), мерцает при ожидании ответа источника и дёргается при отказе.
//!
//! # Подсветка
//! Подсветка не хранится: на каждом кадре активность считается по ФАКТАМ (`light_is_active`, `phone_is_active`).
//! Поэтому тап по кнопке света сам ничего не подсвечивает — подсветка появляется, когда движок подтвердил белое.
//!
//! # Поворот
//! Значки — `rotating_content`: крутятся с устройством вокруг СВОЕГО центра (смещение от центра кнопки
//! прикладывается после поворота). Подписи не вращаются, как у вариантов других карточек.

use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::icons::IconId;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{centered_icon, centered_text};

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::white_attention::WhiteAttention;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::white_config::WhiteLightIndex;
use crate::white_display::{PHONE_LABEL, light_is_active, phone_estimate_text, phone_is_active};

/// Что делает кнопка ряда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowButtonKind {
    /// «Phone»: взять белое телефона.
    Phone,
    /// Кнопка света по номеру в конфиге.
    Light(usize),
}

struct RowButton {
    kind: RowButtonKind,
    button: PressButton,
    content: NodeId,
}

/// Ряд «Phone» + кнопки света.
pub(crate) struct LightsRow {
    buttons: Vec<RowButton>,
    /// Внимание кнопки «Phone» (мерцание ожидания, тряска отказа).
    phone_attention: WhiteAttention,
}

impl LightsRow {
    /// Кнопки создаются спрятанными от касаний; состав — «Phone» и все кнопки света из конфига.
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (card_node, card_rect): (NodeId, Rect),
    ) -> Result<Self, WidgetError> {
        let count = 1 + ctx.config.white.lights.len();
        let mut buttons = Vec::with_capacity(count);
        for index in 0..count {
            let kind = if index == 0 {
                RowButtonKind::Phone
            } else {
                RowButtonKind::Light(index - 1)
            };
            let id = match kind {
                RowButtonKind::Phone => ButtonId::WhitePhone,
                RowButtonKind::Light(light) => ButtonId::WhiteLight(u8::try_from(light).map_err(|_| {
                    WidgetError::Inconsistent(format!("кнопок света {} больше, чем помещается в номер", light + 1))
                })?),
            };
            let rect = ctx.geometry.white_light_slot(index, count);
            let node = kit.add_node(card_node, "white_light", rect, card_rect)?;
            let button = PressButton::attach(kit, id, node, rect, ctx)?;
            let content = kit.add_rotating_content(node, "white_light_icon")?;
            kit.set_interactive(node, false)?;
            buttons.push(RowButton { kind, button, content });
        }
        Ok(Self {
            buttons,
            phone_attention: WhiteAttention::new(ctx),
        })
    }

    /// Тап по кнопке ряда → действие. `false` — событие чужое.
    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let ScreenGesture::Press(id) = event.intent else {
            return Ok(false);
        };
        let kind = match id {
            ButtonId::WhitePhone => RowButtonKind::Phone,
            ButtonId::WhiteLight(index) => RowButtonKind::Light(usize::from(index)),
            _ => return Ok(false),
        };
        let button = self
            .buttons
            .iter_mut()
            .find(|button| button.kind == kind)
            .ok_or_else(|| WidgetError::Inconsistent(format!("кнопки белого {kind:?} нет")))?;
        if button.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
            out.actions.push(match kind {
                RowButtonKind::Phone => ScreenAction::WhitePhoneTapped,
                RowButtonKind::Light(index) => ScreenAction::WhiteLightSelected(WhiteLightIndex(index)),
            });
        }
        Ok(true)
    }

    /// Кнопки ловят касания, только пока карточка открыта.
    pub(crate) fn set_touchable(&mut self, kit: &mut SceneKit, touchable: bool) -> Result<(), WidgetError> {
        for button in &self.buttons {
            kit.set_interactive(button.button.node(), touchable)?;
        }
        Ok(())
    }

    /// Внимание «Phone» следует за фактами каждый кадр (и при закрытой карточке: открытие застанет ожидание).
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) {
        self.phone_attention.sync(ctx);
    }

    pub(crate) fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.phone_attention.is_animating(now)
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        (alpha, slide_px): (f32, f32),
    ) -> Result<(), WidgetError> {
        let white = &ctx.geometry.config().white_card;
        let facts = ctx.state.camera_facts().white;
        let colors = &ctx.tokens.colors;
        for row_button in &self.buttons {
            let (Some(button), Some(content)) = (
                visuals.placement(row_button.button.node()),
                visuals.placement(row_button.content),
            ) else {
                continue;
            };
            let is_phone = row_button.kind == RowButtonKind::Phone;
            let (icon, label, active) = match row_button.kind {
                RowButtonKind::Phone => (IconId::DeviceMobile, PHONE_LABEL, phone_is_active(facts.held)),
                RowButtonKind::Light(index) => {
                    let Some(light) = ctx.config.white.lights.get(index) else {
                        return Err(WidgetError::Inconsistent(format!("кнопки света {index} нет в конфиге")));
                    };
                    (
                        light.icon.icon_id(),
                        light.label.as_str(),
                        light_is_active(light, facts.held.setting, &ctx.config.white),
                    )
                }
            };
            let color = if active {
                colors.focus_yellow
            } else {
                colors.text_primary
            };
            let attention_opacity = if is_phone {
                self.phone_attention.opacity(ctx.now)
            } else {
                1.0
            };
            let shake_pt = if is_phone {
                self.phone_attention.offset_x_pt(ctx.now)
            } else {
                0.0
            };
            let opacity = Opacity::new((button.opacity * alpha * attention_opacity).clamp(0.0, 1.0))
                .map_err(|_| WidgetError::Inconsistent("прозрачность кнопки света вне 0..1".to_owned()))?;
            // Смещения: значок и подписи стоят на своих высотах; тряска — вбок, в осях экрана.
            let offset = |dy_pt: f64| Affine2::translate(ctx.px.px(shake_pt), ctx.px.px(dy_pt) + slide_px);
            layers.push(
                Layer::Content,
                centered_icon(
                    icon,
                    white.light_icon_size_pt,
                    ctx.px,
                    content.transform.then(offset(white.light_icon_offset_pt)),
                    color,
                    opacity,
                ),
            );
            let caption = texts.run(ctx, label, TextRoleKind::Caption)?;
            layers.push(
                Layer::Content,
                centered_text(
                    caption,
                    button.transform.then(offset(white.light_label_offset_pt)),
                    color,
                    opacity,
                ),
            );
            if is_phone {
                self.draw_estimate(
                    ctx,
                    texts,
                    layers,
                    button.transform.then(offset(white.light_estimate_offset_pt)),
                    opacity,
                )?;
            }
        }
        Ok(())
    }

    /// Оценка источника под подписью «Phone»; нет измерения — строки нет (место остаётся пустым).
    fn draw_estimate(
        &self,
        ctx: &Ctx<'_>,
        texts: &mut TextRuns,
        layers: &mut DrawLayers,
        host: Affine2,
        opacity: Opacity,
    ) -> Result<(), WidgetError> {
        let facts = ctx.state.camera_facts().white;
        let Some(estimate) = phone_estimate_text(facts.phone_reading, &ctx.config.white) else {
            return Ok(());
        };
        let muted: DisplayColor = ctx.tokens.colors.text_muted;
        let run = texts.run(ctx, &estimate, TextRoleKind::Caption)?;
        layers.push(Layer::Content, centered_text(run, host, muted, opacity));
        Ok(())
    }
}
