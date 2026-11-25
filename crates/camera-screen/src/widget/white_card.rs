//! Содержимое карточки белого (WB28): «×», числа осей, ряд «Phone» + кнопки света, шкала выбранной оси.
//!
//! # Почему отдельный узел
//! Белая карточка на 24 pt выше обычной, а хит-тест `ui-core` не заходит в детей за границей родителя. Если бы
//! содержимое висело на узле обычной карточки, верхние 24 pt («×» и числа осей) не ловили бы касаний, и тап уходил
//! бы под карточку, в видоискатель, и закрывал панель. Поэтому у белого свой узел с высотой белой карточки; стекло,
//! морф и закрытие по-прежнему ведёт `ToolCardWidget` (он же зовёт этот виджет).
//!
//! # Владение
//! Белое экран не хранит. Ось (`WhiteAxis`) держит `ScreenState`; виджет читает её каждый кадр. Открытие и
//! закрытие карточки, смена оси и показ фактов намерений НЕ порождают: намерение рождают только тап по «Phone» или
//! кнопке света и ведение шкалы пальцем.
//!
//! # Верхняя строка
//! «×» слева, по центру два числа-кнопки — «3140 K» и «Tint +16» (зоны ≥ 44 pt). Выбранная ось — яркая, с короткой
//! чертой-капсулой под числом; другая приглушена. Заголовка «WHITE» нет: в эскизе владельца его нет, а оба числа
//! и «×» в 421 pt его оставляют без места.

use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::centered_text;

use super::button::{ButtonEvent, PressButton};
use super::error::WidgetError;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::side_button::{IconTurn, SideButton, SideButtonLook};
use super::text_runs::TextRuns;
use super::white_card_row::LightsRow;
use super::white_ruler::WhiteRuler;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::white::WhiteAxis;
use crate::white_display::{KELVIN_UNIT, TINT_LABEL_PREFIX, kelvin_text, tint_text};

/// Оси в порядке слева направо.
const AXES: [WhiteAxis; 2] = [WhiteAxis::Temperature, WhiteAxis::Tint];

/// Число-кнопка оси.
struct AxisButton {
    axis: WhiteAxis,
    button: PressButton,
}

/// Содержимое белой карточки.
pub(crate) struct WhiteCardWidget {
    node: NodeId,
    close: SideButton,
    axes: Vec<AxisButton>,
    row: LightsRow,
    ruler: WhiteRuler,
}

impl WhiteCardWidget {
    /// Узел белой карточки — ребёнок `parent` (того же, что у обычной карточки), поверх неё в хит-тесте.
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let rect = ctx.geometry.white_card();
        let node = kit.add_node(parent, "white_card", rect, ctx.geometry.screen())?;
        // Свайп вниз по карточке закрывает её, как и у обычной (решает редьюсер).
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Down,
                ..PanSpec::default()
            }),
            ScreenGesture::DrawerClosePan,
        )?;
        let mut axes = Vec::with_capacity(AXES.len());
        for axis in AXES {
            let frame = ctx.geometry.white_axis_button(axis);
            let axis_node = kit.add_node(node, "white_axis", frame, rect)?;
            let button = PressButton::attach(kit, ButtonId::WhiteAxis(axis), axis_node, frame, ctx)?;
            kit.set_interactive(axis_node, false)?;
            axes.push(AxisButton { axis, button });
        }
        let close = SideButton::build(
            kit,
            ctx,
            (node, rect),
            (ButtonId::WhiteClose, ctx.geometry.white_close_button()),
            IconTurn::FollowsPhone,
        )?;
        let row = LightsRow::build(kit, ctx, (node, rect))?;
        let ruler = WhiteRuler::build(kit, ctx, (node, rect))?;
        kit.set_interactive(node, false)?;
        Ok(Self {
            node,
            close,
            axes,
            row,
            ruler,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        selected: WhiteAxis,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::Press(ButtonId::WhiteClose) => {
                if self.close.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::SubmenuBackRequested);
                }
                Ok(true)
            }
            ScreenGesture::Press(ButtonId::WhiteAxis(axis)) => {
                let Some(button) = self.axes.iter_mut().find(|button| button.axis == axis) else {
                    return Err(WidgetError::Inconsistent(format!("кнопки оси {axis:?} нет")));
                };
                if button.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::WhiteAxisSelected(axis));
                }
                Ok(true)
            }
            _ => Ok(self.row.handle_gesture(event, out)? || self.ruler.handle_gesture(event, selected)?),
        }
    }

    /// Следит за фактами всегда; касания включает, только пока белая карточка открыта.
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>, open: bool) -> Result<(), WidgetError> {
        self.ruler.sync(ctx);
        self.row.sync(ctx);
        kit.set_interactive(self.node, open)?;
        kit.set_interactive(self.close.button.node(), open)?;
        for button in &self.axes {
            kit.set_interactive(button.button.node(), open)?;
        }
        self.row.set_touchable(kit, open)?;
        self.ruler.set_active(kit, open)
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.ruler.advance(ctx, out);
    }

    /// Нужны ли кадры: идёт мерцание или тряска «Phone».
    pub(crate) fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.row.is_animating(now)
    }

    pub(crate) fn invalid_values(&self) -> u64 {
        self.ruler.invalid_values()
    }

    #[cfg(test)]
    pub(crate) fn ruler(&self) -> &WhiteRuler {
        &self.ruler
    }

    /// Содержимое карточки; стекло уже нарисовал `ToolCardWidget`.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        (alpha, slide_px): (f32, f32),
    ) -> Result<(), WidgetError> {
        self.close.draw(
            ctx,
            visuals,
            layers,
            SideButtonLook {
                icon: IconId::Close,
                rotation: Affine2::IDENTITY,
                content_alpha: alpha,
                slide_px,
                extra_opacity: 1.0,
            },
        )?;
        self.draw_axes(ctx, visuals, layers, texts, (alpha, slide_px))?;
        self.row.draw(ctx, visuals, layers, texts, (alpha, slide_px))?;
        self.ruler
            .draw(ctx, visuals, layers, (ctx.state.white().axis(), alpha, slide_px))
    }

    /// Числа осей и черта под выбранной.
    fn draw_axes(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        (alpha, slide_px): (f32, f32),
    ) -> Result<(), WidgetError> {
        let white = &ctx.geometry.config().white_card;
        let facts = ctx.state.camera_facts().white;
        let selected = ctx.state.white().axis();
        let colors = &ctx.tokens.colors;
        for button in &self.axes {
            let Some(placement) = visuals.placement(button.button.node()) else {
                continue;
            };
            let is_selected = button.axis == selected;
            let dim = if is_selected {
                1.0
            } else {
                white.axis_dim_opacity as f32
            };
            let opacity = Opacity::new((placement.opacity * alpha * dim).clamp(0.0, 1.0))
                .map_err(|_| WidgetError::Inconsistent("прозрачность числа оси вне 0..1".to_owned()))?;
            let text = match button.axis {
                WhiteAxis::Temperature => format!(
                    "{} {KELVIN_UNIT}",
                    kelvin_text(facts.held.setting.temperature, &ctx.config.white)
                ),
                WhiteAxis::Tint => format!(
                    "{TINT_LABEL_PREFIX}{}",
                    tint_text(facts.held.setting.tint, &ctx.config.white)
                ),
            };
            let slide = Affine2::translate(0.0, slide_px);
            let host = placement.transform.then(slide);
            let run = texts.run(ctx, &text, TextRoleKind::CardValue)?;
            layers.push(Layer::Content, centered_text(run, host, colors.text_primary, opacity));
            if is_selected {
                let capsule = Affine2::translate(0.0, ctx.px.px(white.axis_underline_offset_pt)).then(host);
                layers.push(
                    Layer::Content,
                    DrawItem::Shape(ShapeItem {
                        geometry: ShapeGeometry::Capsule {
                            size: PxSize::new(
                                ctx.px.px(white.axis_underline_width_pt),
                                ctx.px.px(white.axis_underline_height_pt),
                            ),
                        },
                        transform: capsule,
                        fill: colors.text_primary,
                        stroke: None,
                        shadow: None,
                        opacity,
                    }),
                );
            }
        }
        Ok(())
    }
}
