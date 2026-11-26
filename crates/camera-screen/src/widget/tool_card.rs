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
//! * Белая карточка (WB28) на 24 pt выше: её содержимое и касания ведёт отдельный виджет (`white_card.rs`) со своим
//!   узлом, а здесь остаётся стекло. Рамка стекла опускается/поднимается на «высоту» узла-метки `extra`, которую
//!   двигает та же пружина и в тот же кадр, что и подъём затвора, — поэтому при смене «EV ↔ белое» значком
//!   затвор не наезжает на карточку ни в один момент.
//! * Рисуется карточка поверх шторки (см. `Stage::draw`): стекло, затем содержимое.

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_draw::color::Opacity;
use ui_draw::draw_list::DrawItem;
use ui_draw::icons::IconId;
use ui_draw::transform::Affine2;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase};
use ui_kit::tokens::{GlassRole, MotionToken, TextRoleKind};
use ui_kit::view::{centered_text, panel_glass_group};

use super::button::ButtonEvent;
use super::chip_row::RowPaint;
use super::error::{WidgetError, element};
use super::exposure_ruler::ExposureRuler;
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::side_button::{IconTurn, SideButton, SideButtonLook};
use super::text_runs::{TextRuns, displayable_text};
use super::tool_card_content::{
    card_value, current_option, has_option_row, open_card_tool, option_entries, tool_title,
};
use super::tool_card_options::OptionRow;
use super::tool_card_styles::{StylesRow, active_preset_name, styles_have_edits};
use super::white_card::WhiteCardWidget;
use super::white_card_site::WhiteCardSite;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::settings::CaptureSettings;
use crate::tools::ToolId;
use crate::white::WhiteAxis;

/// Пружина морфа шторка → карточка и закрытия: без перелёта, ≈ 0,3 с `[measured: C t=37.8–38.1, 62.93–63.25]`.
pub(crate) const MENU_MORPH_MOTION: MotionToken = MotionToken::CardMorph;
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

/// Что случилось с открытием карточки в этом кадре (для узла-метки высоты стекла).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtraHeightEvent {
    /// Ничего нового: смена высоты (если она есть) идёт пружиной.
    Unchanged,
    /// Карточка только что открылась: высота встаёт сразу, вместе с проявлением стекла.
    JustOpened,
}

/// Показанная сейчас карточка: нужна и во время закрытия, когда состояние уже забыло об инструменте.
#[derive(Debug, Clone)]
struct ShownCard {
    tool: ToolId,
    left_face: LeftButtonFace,
    /// Крупное значение, собранное по `value_key`.
    value: String,
    value_key: ValueKey,
    /// Имя активного пресета, из которого собрано `value` (только STYLES): по нему видно, что имя сменилось.
    value_source: String,
}

pub(crate) struct ToolCardWidget {
    node: NodeId,
    left: SideButton,
    right: SideButton,
    ruler: ExposureRuler,
    options: OptionRow,
    /// Ряд имён пресетов карточки STYLES (решение 201).
    styles: StylesRow,
    /// Морф рамки; `None`, пока карточка ни разу не открывалась или полностью закрылась.
    morph: Option<PanelMorph>,
    /// Вид текущего морфа (`Some` ровно тогда, когда есть `morph`).
    morph_style: Option<MorphStyle>,
    /// К открытию ли едем сейчас.
    open_wanted: bool,
    shown: Option<ShownCard>,
    /// Содержимое белой карточки (свой узел высотой в белую карточку).
    white: WhiteCardWidget,
    /// Ось, выбранная на белой карточке (читается из состояния в `sync`, чтобы жест не ходил за состоянием).
    white_axis: WhiteAxis,
    /// Узел-метка: его сдвиг по вертикали = на сколько рамка стекла выше обычной (минус pt). Никаких касаний.
    extra_node: NodeId,
    /// Какая «лишняя» высота стекла назначена сейчас, pt (0 — обычная карточка).
    extra_target_pt: f64,
    /// Текущий сдвиг метки, pt (≤ 0): рамка стекла поднята на столько. Обновляется в `sync` после анимаций.
    extra_offset_pt: f64,
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
        let widgets = &ctx.config.calibration.widgets;
        let (styles_center, styles_width) = ctx.geometry.card_chip_row(widgets.chip_padding_pt);
        let styles = StylesRow::build(kit, ctx, (node, card_rect), styles_center, styles_width)?;
        let left = SideButton::build(
            kit,
            ctx,
            (node, card_rect),
            (ButtonId::CardLeft, ctx.geometry.card_left_button()),
            IconTurn::FollowsPhone,
        )?;
        let right = SideButton::build(
            kit,
            ctx,
            (node, card_rect),
            (ButtonId::CardRight, ctx.geometry.card_right_button()),
            IconTurn::FollowsPhone,
        )?;
        kit.set_interactive(node, false)?;
        // Белая карточка и метка высоты — после обычной: выше в хит-тесте.
        let white = WhiteCardWidget::build(kit, ctx, parent, WhiteCardSite::photo(ctx.geometry))?;
        let extra_node = kit.add_node(parent, "card_extra", card_rect, ctx.geometry.screen())?;
        kit.set_interactive(extra_node, false)?;
        Ok(Self {
            node,
            left,
            right,
            ruler,
            options,
            styles,
            morph: None,
            morph_style: None,
            open_wanted: false,
            shown: None,
            white,
            white_axis: WhiteAxis::default(),
            extra_node,
            extra_target_pt: 0.0,
            extra_offset_pt: 0.0,
        })
    }

    pub(crate) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
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
                    // Что значит «↻», решает инструмент карточки: сброс EV или возврат пресета к исходным значениям.
                    match self.shown.as_ref().map(|shown| shown.tool) {
                        Some(ToolId::Styles) => out.actions.push(ScreenAction::DiscardEditsTapped),
                        _ => out.actions.push(ScreenAction::ExposureResetRequested),
                    }
                }
                Ok(true)
            }
            _ => Ok(self.options.handle_gesture(event, out)?
                || self.styles.handle_gesture(kit, event, &mut out.actions)?
                || self.ruler.handle_gesture(event)?
                || self.white.handle_gesture(kit, event, self.white_axis, out)?),
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
        self.white_axis = ctx.state.white().axis();
        let open_tool = open_card_tool(ctx.state);
        // Шкалы белого следуют за фактами всегда; касания включены, только пока открыта именно белая карточка.
        self.white.sync(kit, ctx, open_tool == Some(ToolId::White))?;
        let Some(tool) = open_tool else {
            if self.open_wanted {
                self.open_wanted = false;
                self.close(ctx)?;
            }
            self.sync_extra_height(kit, ctx, ExtraHeightEvent::Unchanged)?;
            return self.make_untouchable(kit);
        };
        let mut height_event = ExtraHeightEvent::Unchanged;
        if !self.open_wanted {
            self.open_wanted = true;
            self.open(ctx)?;
            height_event = ExtraHeightEvent::JustOpened;
        }
        self.sync_extra_height(kit, ctx, height_event)?;
        let left_face = if ctx.state.card_returns_to_menu() {
            LeftButtonFace::BackToMenu
        } else {
            LeftButtonFace::Close
        };
        self.refresh_shown(ctx, tool, left_face);
        // У белой карточки свой узел и своя «×»: узел обычной карточки под ней касаний не ловит.
        let standard_card = tool != ToolId::White;
        kit.set_interactive(self.node, standard_card)?;
        kit.set_interactive(self.left.button.node(), standard_card)?;
        let is_exposure = tool == ToolId::Exposure;
        self.ruler.set_active(kit, is_exposure)?;
        let is_styles = tool == ToolId::Styles;
        // «↻» при EV = 0 (или без правок поверх пресета) сбрасывать нечего: кнопка затемнена и не ловит касаний.
        kit.set_interactive(
            self.right.button.node(),
            (is_exposure && ctx.state.exposure().steps() != 0) || (is_styles && styles_have_edits(ctx.state)),
        )?;
        if is_styles {
            self.styles.sync(kit, ctx)?;
        } else {
            self.styles.stop_touches(kit)?;
        }
        self.sync_options(kit, ctx, tool)
    }

    /// Выравнивает «лишнюю» высоту стекла по открытой карточке. Закрывающаяся карточка держит последнюю высоту
    /// (стекло доисчезает тем же, чем было). Смена «обычная ↔ белая» при открытой карточке идёт пружиной подъёма
    /// затвора в тот же кадр, при Reduce Motion — сразу.
    fn sync_extra_height(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        event: ExtraHeightEvent,
    ) -> Result<(), WidgetError> {
        if let Some(tool) = open_card_tool(ctx.state) {
            let desired_pt = ctx.geometry.card_extra_height_pt(Some(tool));
            let changed = desired_pt.to_bits() != self.extra_target_pt.to_bits();
            self.extra_target_pt = desired_pt;
            if event == ExtraHeightEvent::JustOpened || (changed && ctx.motion == MotionPreference::Reduced) {
                kit.jump_vertically_to(self.extra_node, -desired_pt)?;
            } else if changed {
                kit.slide_vertically_to(self.extra_node, -desired_pt, ctx.spring(MENU_MORPH_MOTION)?, ctx.now)?;
            }
        }
        self.extra_offset_pt = kit.vertical_offset_pt(self.extra_node)?;
        Ok(())
    }

    /// Рамка стекла с учётом «лишней» высоты: верх поднят на `lift`, низ на месте.
    fn lifted_frame(&self, frame: Rect) -> Rect {
        let lift_pt = (-self.extra_offset_pt).max(0.0);
        Rect::from_xywh(
            frame.min_x(),
            frame.min_y() - lift_pt,
            frame.size.width,
            frame.size.height + lift_pt,
        )
    }

    /// Доля роста белой карточки, 0..1: содержимое белой карточки проявляется вместе с растущим стеклом.
    fn white_growth(&self) -> f32 {
        if self.extra_target_pt <= 0.0 {
            return 1.0;
        }
        ((-self.extra_offset_pt) / self.extra_target_pt).clamp(0.0, 1.0) as f32
    }

    /// Обновляет запомненную карточку. Строка значения пересобирается только при смене ключа.
    fn refresh_shown(&mut self, ctx: &Ctx<'_>, tool: ToolId, left_face: LeftButtonFace) {
        let value_key = ValueKey {
            tool,
            settings: *ctx.state.settings(),
            exposure_steps: ctx.state.exposure().steps(),
        };
        // Имя активного пресета живёт в фактах, а не в настройках съёмки: оно может смениться (переименование,
        // выбор из другого места), не тронув ключ. Поэтому у STYLES значение сверяется по самому имени.
        let value_source = if tool == ToolId::Styles {
            active_preset_name(ctx.state)
        } else {
            String::new()
        };
        let (value, value_source) = match self.shown.take() {
            Some(previous)
                if tool == ToolId::Styles && previous.tool == tool && previous.value_source == value_source =>
            {
                (previous.value, value_source)
            }
            _ if tool == ToolId::Styles => (displayable_text(ctx, &value_source), value_source),
            Some(previous) if previous.value_key == value_key => (previous.value, value_source),
            _ => (card_value(ctx.state, ctx.config, tool), value_source),
        };
        self.shown = Some(ShownCard {
            tool,
            left_face,
            value,
            value_key,
            value_source,
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
        self.styles.stop_touches(kit)?;
        self.options.stop_touches(kit)
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.ruler.advance(ctx, out);
        self.white.advance(ctx, out);
        if let Some(morph) = &mut self.morph {
            morph.advance(ctx.now);
            let _lifecycle = morph.drain_events();
            if !self.open_wanted && morph.phase(ctx.now) == PanelPhase::Closed {
                // Закрылась целиком: морф и запомненный инструмент больше не нужны.
                self.morph = None;
                self.morph_style = None;
                self.shown = None;
                self.options.forget_entries();
                self.styles.forget();
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
        let mut snapshot = morph.snapshot(ctx.now);
        if snapshot.phase == PanelPhase::Closed {
            return Ok(());
        }
        snapshot.frame = self.lifted_frame(snapshot.frame);
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
        if shown.tool == ToolId::White {
            // У белой карточки своё содержимое (заголовка и крупного значения нет): растёт вместе со стеклом.
            let growth = self.white_growth();
            return self.white.draw(ctx, visuals, layers, texts, (alpha * growth, slide_px));
        }
        self.draw_texts(ctx, layers, texts, shown, alpha, slide_px)?;
        if shown.tool == ToolId::Exposure {
            self.ruler.draw(ctx, visuals, layers, alpha, slide_px)?;
        } else if shown.tool == ToolId::Styles {
            self.styles
                .draw(ctx, visuals, layers, texts, RowPaint { alpha, slide_px })?;
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
        let reset_nothing_to_reset = match shown.tool {
            ToolId::Exposure => Some(ctx.state.exposure().steps() == 0),
            ToolId::Styles => Some(!styles_have_edits(ctx.state)),
            _ => None,
        };
        if let Some(nothing_to_reset) = reset_nothing_to_reset {
            let dim = if nothing_to_reset {
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

    /// Идёт ли мерцание или тряска на карточке (кадры нужны, пока они не кончились).
    pub(crate) fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.white.is_animating(now)
    }

    /// Сколько значений или пределов шкал белого отвергнуто как невозможные.
    pub(crate) fn invalid_white_values(&self) -> u64 {
        self.white.invalid_values()
    }

    #[cfg(test)]
    pub(crate) fn white(&self) -> &WhiteCardWidget {
        &self.white
    }

    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.ruler.suppressed_echo_events()
    }

    /// Имена пресетов, не поместившиеся в ряд карточки STYLES (диагностика).
    pub(crate) fn overflowed_style_chips(&self) -> usize {
        self.styles.overflowed_chips()
    }
}
