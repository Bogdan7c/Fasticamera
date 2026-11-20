//! Нижняя карточка инструмента (решение 103): ОДИН элемент для Timer, Flash, Aspect, RAW, Grid, Level и Exposure.
//!
//! Карточка заменила строки подменю внутри шторки Controls и однострочную панель Exposure. Состав:
//! стеклянная рамка (K7 `PanelMorph`), заголовок ЗАГЛАВНЫМИ, крупное значение, левая кнопка («‹» назад в меню
//! или «×» закрыть), правая кнопка («↻» сброс, только Exposure) и либо ряд вариантов (`tool_card_options.rs`),
//! либо линейка EV (`exposure_ruler.rs`). Что написано — решает `tool_card_content.rs`, где стоит — геометрия
//! `card_geometry.rs`.
//!
//! # Владение и инварианты
//! * Какая карточка открыта и откуда — владеет `ScreenState` (панель и происхождение); виджет только
//!   выравнивается по нему в `sync` (без эха действий). Заголовок, значение и подсветка выбранного берутся из
//!   состояния на каждом кадре, поэтому после выбора варианта карточка показывает новую настройку.
//! * Пока карточка закрывается, состояние уже `Closed`: инструмент и вид левой кнопки запоминаются в
//!   `ShownCard`, чтобы содержимое доисчезало тем же, чем было.
//! * Морф выбирается при открытии по происхождению: из меню — шторка → карточка, со значка — на месте.
//!   Морф, дошедший до закрытия, но ещё идущий, переиспользуется только если вид тот же ([`MorphStyle`]);
//!   иначе (меню ↔ значок посреди закрытия) собирается новый.
//! * Варианты ряда гаснут вместе с карточкой: касания снимаются сразу, сами варианты забываются, когда морф
//!   дошёл до `Closed` (`advance`).
//! * Рисуется карточка поверх шторки (см. `Stage::draw`): стекло, затем содержимое.

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase};
use ui_kit::tokens::{GlassRole, MotionToken, TextRoleKind};
use ui_kit::view::{centered_icon, centered_text, panel_glass_group};

use super::button::{ButtonEvent, PressButton};
use super::error::{WidgetError, element};
use super::exposure_ruler::ExposureRuler;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::tool_card_content::{
    card_value, current_option, has_option_row, open_card_tool, option_entries, tool_title,
};
use super::tool_card_options::OptionRow;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::settings::CaptureSettings;
use crate::tools::ToolId;

/// Пружина морфа шторка → карточка и закрытия: без перелёта, ≈ 0,3 с `[measured: C t=37.8–38.1, 62.93–63.25]`.
const MENU_MORPH_MOTION: MotionToken = MotionToken::CardMorph;
/// Пружина проявления карточки на месте (открытие со значка над кадром): ≈ 0,2 с `[measured: C t=47.93–48.12]`.
/// Закрытие такой карточки идёт той же пружиной (одна пружина на морф): `[estimate]` 0,2 вместо измеренных 0,3 с.
const STRIP_APPEAR_MOTION: MotionToken = MotionToken::CardAppear;

/// Что делает левая кнопка карточки (определяется происхождением).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeftButtonFace {
    /// «‹»: назад в меню Controls.
    BackToMenu,
    /// «×»: закрыть панель.
    Close,
}

/// Вид морфа карточки (по происхождению): от него зависит, можно ли переиспользовать идущий морф.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MorphStyle {
    /// Шторка вырастает в карточку (открыта из меню Controls).
    FromSheet,
    /// Карточка проявляется на месте (открыта значком над кадром).
    InPlace,
}

/// От чего зависит крупное значение: пока ключ не изменился, строка значения не пересобирается (`format!`
/// на каждом кадре — лишняя аллокация).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ValueKey {
    tool: ToolId,
    settings: CaptureSettings,
    exposure_steps: i32,
}

/// Показанная сейчас карточка: нужна и во время закрытия, когда состояние уже забыло об инструменте.
#[derive(Debug, Clone)]
struct ShownCard {
    tool: ToolId,
    left_face: LeftButtonFace,
    /// Крупное значение, собранное по `value_key`.
    value: String,
    value_key: ValueKey,
}

/// Как рисуется боковая кнопка в этом кадре.
#[derive(Debug, Clone, Copy)]
struct SideButtonLook {
    icon: IconId,
    /// Поворот значка (стрелка «‹» смотрит влево; «×» и «↻» не поворачиваются).
    rotation: Affine2,
    /// Прозрачность содержимого карточки из морфа.
    content_alpha: f32,
    /// Сдвиг содержимого карточки из морфа, px.
    slide_px: f32,
    /// Дополнительный множитель прозрачности (затемнение «сброса» при EV = 0); 1,0 — без затемнения.
    extra_opacity: f32,
}

/// Круглая боковая кнопка: кнопка и узел значка (поворачивается по ориентации телефона).
struct SideButton {
    button: PressButton,
    content: NodeId,
}

impl SideButton {
    fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        card_node: NodeId,
        card_rect: Rect,
        (id, rect): (ButtonId, Rect),
    ) -> Result<Self, WidgetError> {
        let node = kit.add_node(card_node, "card_button", rect, card_rect)?;
        let button = PressButton::attach(kit, id, node, rect, ctx)?;
        let content = kit.add_rotating_content(node, "card_button_icon")?;
        // Диск Ø 36 рисуется как есть, а попасть в него можно в квадрат ≥ 44 pt (HIG).
        kit.ensure_min_touch_side(node, rect, ctx.geometry.config().card.button_touch_side_pt)?;
        kit.set_interactive(node, false)?;
        Ok(Self { button, content })
    }

    /// Чёрный диск и значок поверх.
    fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        look: SideButtonLook,
    ) -> Result<(), WidgetError> {
        let SideButtonLook {
            icon,
            rotation,
            content_alpha,
            slide_px,
            extra_opacity,
        } = look;
        let (Some(button), Some(content)) = (visuals.placement(self.button.node()), visuals.placement(self.content))
        else {
            return Ok(());
        };
        let alpha = (button.opacity * content_alpha * extra_opacity).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Ok(());
        }
        let opacity = Opacity::new(alpha)
            .map_err(|_| WidgetError::Inconsistent("прозрачность кнопки карточки вне 0..1".to_owned()))?;
        let card = &ctx.geometry.config().card;
        let slide = Affine2::translate(0.0, slide_px);
        layers.push(
            Layer::Content,
            DrawItem::Shape(ShapeItem {
                geometry: ShapeGeometry::Circle {
                    diameter: button.size.width.min(button.size.height),
                },
                transform: slide.then(button.transform),
                fill: DisplayColor {
                    a: card.button_disc_alpha,
                    ..DisplayColor::BLACK
                },
                stroke: None,
                shadow: None,
                opacity,
            }),
        );
        let content = NodePlacementPx {
            transform: rotation.then(content.transform),
            ..content
        };
        layers.push(
            Layer::Content,
            centered_icon(
                icon,
                card.button_icon_size_pt,
                ctx.px,
                slide.then(content.transform),
                ctx.tokens.colors.text_primary,
                opacity,
            ),
        );
        Ok(())
    }
}

pub(crate) struct ToolCardWidget {
    node: NodeId,
    left: SideButton,
    right: SideButton,
    ruler: ExposureRuler,
    options: OptionRow,
    /// Морф рамки; `None`, пока карточка ни разу не открывалась или полностью закрылась.
    morph: Option<PanelMorph>,
    /// Вид текущего морфа (`Some` ровно тогда, когда есть `morph`).
    morph_style: Option<MorphStyle>,
    /// К открытию ли едем сейчас.
    open_wanted: bool,
    shown: Option<ShownCard>,
}

impl ToolCardWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        let card_rect = ctx.geometry.card();
        let node = kit.add_node(parent, "tool_card", card_rect, ctx.geometry.screen())?;
        // Свайп вниз по карточке — то же закрытие, что и по шторке (решает редьюсер по происхождению).
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: PanDirection::Down,
                ..PanSpec::default()
            }),
            ScreenGesture::DrawerClosePan,
        )?;
        let ruler = ExposureRuler::build(kit, ctx, node, card_rect)?;
        let options = OptionRow::build(kit, ctx, node, card_rect)?;
        let left = SideButton::build(
            kit,
            ctx,
            node,
            card_rect,
            (ButtonId::CardLeft, ctx.geometry.card_left_button()),
        )?;
        let right = SideButton::build(
            kit,
            ctx,
            node,
            card_rect,
            (ButtonId::CardRight, ctx.geometry.card_right_button()),
        )?;
        kit.set_interactive(node, false)?;
        Ok(Self {
            node,
            left,
            right,
            ruler,
            options,
            morph: None,
            morph_style: None,
            open_wanted: false,
            shown: None,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::Press(ButtonId::CardLeft) => {
                if self.left.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::SubmenuBackRequested);
                }
                Ok(true)
            }
            ScreenGesture::Press(ButtonId::CardRight) => {
                if self.right.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::ExposureResetRequested);
                }
                Ok(true)
            }
            _ => Ok(self.options.handle_gesture(event, out)? || self.ruler.handle_gesture(event)?),
        }
    }

    /// Насколько карточка закрыла собой шторку меню, 0..1: шторка гасит своё стекло и пункты на эту долю
    /// (стекло не на стекле). Считается только для морфа «из шторки»: карточка, проявляющаяся на месте со
    /// значка, шторку не закрывает (её под ней нет). Без карточки — 0.
    pub(crate) fn cover_progress(&self, now: MonotonicNanos) -> f32 {
        match (&self.morph, self.morph_style) {
            (Some(morph), Some(MorphStyle::FromSheet)) => (morph.snapshot(now).progress as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Вид морфа по происхождению открытой карточки.
    fn desired_style(ctx: &Ctx<'_>) -> MorphStyle {
        if ctx.state.card_returns_to_menu() {
            MorphStyle::FromSheet
        } else {
            MorphStyle::InPlace
        }
    }

    /// Морф вида `style`: из меню шторка вырастает в карточку, со значка карточка проявляется на месте.
    fn new_morph(ctx: &Ctx<'_>, style: MorphStyle) -> Result<PanelMorph, WidgetError> {
        let target = ctx.geometry.card();
        let radius = ctx.geometry.config().card.corner_radius_pt;
        let spec = if style == MorphStyle::FromSheet {
            // Шторка закруглена радиусом 54, а не «круглой кнопкой» — стартовый радиус задаём явно.
            PanelMorphSpec::new(ctx.geometry.sheet(), target, radius)
                .with_source_corner_radius_pt(ctx.geometry.config().sheet.corner_radius_pt)
                .with_spring(MENU_MORPH_MOTION)
        } else {
            PanelMorphSpec::in_place(target, radius, STRIP_APPEAR_MOTION)
        };
        PanelMorph::new(spec, &ctx.environment(target.size.height), ctx.now).map_err(element("карточка инструмента"))
    }

    fn open(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let style = Self::desired_style(ctx);
        // Идущий морф переиспользуем (открытие посреди закрытия — разворот с сохранением скорости), но только
        // если вид тот же: морф «из шторки» не годится для карточки на месте и наоборот.
        let reusable = self.morph_style == Some(style)
            && self
                .morph
                .as_ref()
                .is_some_and(|morph| morph.phase(ctx.now) != PanelPhase::Closed);
        if !reusable {
            self.morph = Some(Self::new_morph(ctx, style)?);
            self.morph_style = Some(style);
        }
        if let Some(morph) = &mut self.morph {
            morph.open(ctx.now).map_err(element("карточка инструмента"))?;
        }
        Ok(())
    }

    fn close(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        if let Some(morph) = &mut self.morph {
            morph.close(ctx.now).map_err(element("карточка инструмента"))?;
        }
        Ok(())
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.ruler.sync(ctx)?;
        let Some(tool) = open_card_tool(ctx.state) else {
            if self.open_wanted {
                self.open_wanted = false;
                self.close(ctx)?;
            }
            return self.make_untouchable(kit);
        };
        if !self.open_wanted {
            self.open_wanted = true;
            self.open(ctx)?;
        }
        let left_face = if ctx.state.card_returns_to_menu() {
            LeftButtonFace::BackToMenu
        } else {
            LeftButtonFace::Close
        };
        self.refresh_shown(ctx, tool, left_face);
        kit.set_interactive(self.node, true)?;
        kit.set_interactive(self.left.button.node(), true)?;
        let is_exposure = tool == ToolId::Exposure;
        self.ruler.set_active(kit, is_exposure)?;
        // «↻» при EV = 0 сбрасывать нечего: кнопка затемнена и не ловит касаний.
        kit.set_interactive(
            self.right.button.node(),
            is_exposure && ctx.state.exposure().steps() != 0,
        )?;
        self.sync_options(kit, ctx, tool)
    }

    /// Обновляет запомненную карточку. Строка значения пересобирается только при смене ключа.
    fn refresh_shown(&mut self, ctx: &Ctx<'_>, tool: ToolId, left_face: LeftButtonFace) {
        let value_key = ValueKey {
            tool,
            settings: *ctx.state.settings(),
            exposure_steps: ctx.state.exposure().steps(),
        };
        let value = match self.shown.take() {
            Some(previous) if previous.value_key == value_key => previous.value,
            _ => card_value(ctx.state, ctx.config, tool),
        };
        self.shown = Some(ShownCard {
            tool,
            left_face,
            value,
            value_key,
        });
    }

    /// Ряд вариантов следует за инструментом; у Exposure ряда нет. Ряд строится только при смене инструмента
    /// (или возврате касаний после закрытия), а не на каждом кадре.
    fn sync_options(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>, tool: ToolId) -> Result<(), WidgetError> {
        if !has_option_row(tool) {
            if self.options.is_bound() {
                self.options.unbind(kit)?;
            }
        } else if !self.options.is_bound_to(tool) || !self.options.is_touchable() {
            let entries = option_entries(ctx.config, tool);
            self.options.bind(kit, ctx, ctx.geometry.card(), tool, entries)?;
        }
        Ok(())
    }

    /// Карточка закрыта или закрывается: касания проходят мимо, пока она растворяется. Варианты ряда остаются
    /// нарисованными и гаснут вместе с карточкой; забудет их `advance`, когда морф дойдёт до `Closed`.
    fn make_untouchable(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        kit.set_interactive(self.node, false)?;
        kit.set_interactive(self.left.button.node(), false)?;
        kit.set_interactive(self.right.button.node(), false)?;
        self.ruler.set_active(kit, false)?;
        self.options.stop_touches(kit)
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.ruler.advance(ctx, out);
        if let Some(morph) = &mut self.morph {
            morph.advance(ctx.now);
            let _lifecycle = morph.drain_events();
            if !self.open_wanted && morph.phase(ctx.now) == PanelPhase::Closed {
                // Закрылась целиком: морф и запомненный инструмент больше не нужны.
                self.morph = None;
                self.morph_style = None;
                self.shown = None;
                self.options.forget_entries();
            }
        }
    }

    /// Стекло рамки, затем содержимое (содержимое после своего стекла).
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let (Some(morph), Some(shown)) = (&self.morph, &self.shown) else {
            return Ok(());
        };
        let snapshot = morph.snapshot(ctx.now);
        if snapshot.phase == PanelPhase::Closed {
            return Ok(());
        }
        layers.push(
            Layer::Glass,
            DrawItem::Glass(panel_glass_group(
                &snapshot,
                ctx.tokens.glass.style_for(GlassRole::Panel),
                ctx.px,
            )?),
        );
        let alpha = snapshot.content_opacity.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Ok(());
        }
        let slide_px = ctx.px.px(snapshot.content_offset_pt);
        self.draw_texts(ctx, layers, texts, shown, alpha, slide_px)?;
        if shown.tool == ToolId::Exposure {
            self.ruler.draw(ctx, visuals, layers, alpha, slide_px)?;
        } else {
            self.options.draw(
                ctx,
                visuals,
                layers,
                texts,
                current_option(ctx.state, shown.tool),
                (alpha, slide_px),
            )?;
        }
        self.draw_buttons(ctx, visuals, layers, shown, alpha, slide_px)
    }

    /// Заголовок и крупное значение по центру карточки.
    fn draw_texts(
        &self,
        ctx: &Ctx<'_>,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        shown: &ShownCard,
        alpha: f32,
        slide_px: f32,
    ) -> Result<(), WidgetError> {
        let opacity =
            Opacity::new(alpha).map_err(|_| WidgetError::Inconsistent("прозрачность карточки вне 0..1".to_owned()))?;
        let colors = &ctx.tokens.colors;
        let at = |x_pt: f64, y_pt: f64| Affine2::translate(ctx.px.px(x_pt), ctx.px.px(y_pt) + slide_px);
        let title_at = ctx.geometry.card_title_center();
        let title = texts.run(ctx, tool_title(shown.tool), TextRoleKind::CardTitle)?;
        layers.push(
            Layer::Content,
            centered_text(title, at(title_at.x, title_at.y), colors.text_muted, opacity),
        );
        let value_at = ctx.geometry.card_value_center();
        let value = texts.run(ctx, &shown.value, TextRoleKind::CardValue)?;
        layers.push(
            Layer::Content,
            centered_text(value, at(value_at.x, value_at.y), colors.text_primary, opacity),
        );
        Ok(())
    }

    fn draw_buttons(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        shown: &ShownCard,
        alpha: f32,
        slide_px: f32,
    ) -> Result<(), WidgetError> {
        // «‹» — стрелка вверх, повёрнутая на 90° против часовой (смотрит влево); «×» не поворачивается.
        let (left_icon, left_rotation) = match shown.left_face {
            LeftButtonFace::BackToMenu => (IconId::MenuChevron, Affine2::rotate(-std::f32::consts::FRAC_PI_2)),
            LeftButtonFace::Close => (IconId::Close, Affine2::IDENTITY),
        };
        self.left.draw(
            ctx,
            visuals,
            layers,
            SideButtonLook {
                icon: left_icon,
                rotation: left_rotation,
                content_alpha: alpha,
                slide_px,
                extra_opacity: 1.0,
            },
        )?;
        if shown.tool == ToolId::Exposure {
            let dim = if ctx.state.exposure().steps() == 0 {
                ctx.geometry.config().card.reset_dim_opacity
            } else {
                1.0
            };
            self.right.draw(
                ctx,
                visuals,
                layers,
                SideButtonLook {
                    icon: IconId::Reset,
                    rotation: Affine2::IDENTITY,
                    content_alpha: alpha,
                    slide_px,
                    extra_opacity: dim,
                },
            )?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn ruler_value_ev(&self, now: MonotonicNanos) -> f64 {
        self.ruler.value_ev(now)
    }

    pub(crate) fn invalid_bias_values(&self) -> u64 {
        self.ruler.invalid_bias_values()
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.ruler.suppressed_echo_events()
    }
}
