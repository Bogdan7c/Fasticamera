//! Меню Controls: шторка вырастает из кнопки меню (K7 `PanelMorph`), сетка 3 × 3 круглых пунктов из
//! `menu_tools()`; пункт морфится в подменю (K7 пружиной `smooth` + K5 `OptionPicker` для выбора варианта).
//!
//! Закрытие — свайп вниз по шторке или тап по видоискателю (его решает редьюсер). Затвор и режимы под
//! шторкой скрывают сами (решение 94, `motion.md` 4.1).

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, GestureState, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase, PanelSnapshot};
use ui_kit::picker::{LabelId, OptionIcon, OptionPicker, PickerOption};
use ui_kit::tokens::{GlassRole, MotionToken, TextRoleKind};
use ui_kit::view::{centered_icon, centered_text, panel_glass_group};

use super::button::{ButtonEvent, PressButton};
use super::error::{WidgetError, element};
use super::geometry::{MAX_MENU_ITEMS, MAX_SUBMENU_OPTIONS};
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::TextRuns;
use super::{Ctx, Outputs, draw_plain_icon_button};
use crate::action::ScreenAction;
use crate::settings::{AspectRatio, FlashMode, GridSetting, LevelSetting, RawCapture, TimerSetting};
use crate::state::PanelState;
use crate::tools::{ToolId, ToolOption};

struct ItemSlot {
    button: PressButton,
    content: NodeId,
    tool: Option<ToolId>,
}

struct OptionSlot {
    button: PressButton,
    option: Option<ToolOption>,
}

/// Открытое подменю: его морф и список вариантов.
struct Submenu {
    tool: ToolId,
    morph: PanelMorph,
    picker: OptionPicker<ToolOption>,
    /// Подписи вариантов в порядке списка.
    labels: Vec<String>,
}

pub(crate) struct ControlsDrawerWidget {
    sheet_node: NodeId,
    sheet_morph: PanelMorph,
    sheet_open_wanted: bool,
    items: Vec<ItemSlot>,
    options: Vec<OptionSlot>,
    back: PressButton,
    back_content: NodeId,
    submenu: Option<Submenu>,
}

/// Подпись инструмента под кругом пункта.
fn tool_caption(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Flash => "Flash",
        ToolId::Raw => "RAW",
        ToolId::Timer => "Timer",
        ToolId::Exposure => "Exposure",
        ToolId::Aspect => "Aspect",
        ToolId::Grid => "Grid",
        ToolId::Level => "Level",
        ToolId::Live => "Live",
        ToolId::Styles => "Styles",
        ToolId::Night => "Night",
    }
}

/// Включён ли инструмент (жёлтый круг): отличается от значения по умолчанию.
fn tool_is_active(ctx: &Ctx<'_>, tool: ToolId) -> bool {
    let settings = ctx.state.settings();
    match tool {
        ToolId::Flash => settings.flash == FlashMode::On,
        ToolId::Raw => settings.raw == RawCapture::On,
        ToolId::Timer => settings.timer != TimerSetting::Off,
        ToolId::Exposure => ctx.state.exposure().steps() != 0,
        ToolId::Aspect => settings.aspect != AspectRatio::FourByThree,
        ToolId::Grid => settings.grid == GridSetting::On,
        ToolId::Level => settings.level == LevelSetting::On,
        ToolId::Live | ToolId::Styles | ToolId::Night => false,
    }
}

/// Варианты подменю инструмента и подписи к ним. Выбор «Auto» вспышки не предлагается (инвариант 6).
fn options_for(ctx: &Ctx<'_>, tool: ToolId) -> Vec<(ToolOption, String)> {
    let seconds = |setting: TimerSetting| {
        ctx.config
            .timer
            .seconds_for(setting)
            .map_or_else(|| "Off".to_owned(), |value| format!("{value}s"))
    };
    match tool {
        ToolId::Flash => vec![
            (ToolOption::Flash(FlashMode::Off), "Off".to_owned()),
            (ToolOption::Flash(FlashMode::On), "On".to_owned()),
        ],
        ToolId::Raw => vec![
            (ToolOption::Raw(RawCapture::Off), "Off".to_owned()),
            (ToolOption::Raw(RawCapture::On), "On".to_owned()),
        ],
        ToolId::Timer => [
            TimerSetting::Off,
            TimerSetting::Seconds3,
            TimerSetting::Seconds5,
            TimerSetting::Seconds10,
        ]
        .into_iter()
        .map(|setting| (ToolOption::Timer(setting), seconds(setting)))
        .collect(),
        ToolId::Aspect => vec![
            (ToolOption::Aspect(AspectRatio::FourByThree), "4:3".to_owned()),
            (ToolOption::Aspect(AspectRatio::SixteenByNine), "16:9".to_owned()),
            (ToolOption::Aspect(AspectRatio::Square), "1:1".to_owned()),
        ],
        ToolId::Grid => vec![
            (ToolOption::Grid(GridSetting::Off), "Off".to_owned()),
            (ToolOption::Grid(GridSetting::On), "On".to_owned()),
        ],
        ToolId::Level => vec![
            (ToolOption::Level(LevelSetting::Off), "Off".to_owned()),
            (ToolOption::Level(LevelSetting::On), "On".to_owned()),
        ],
        // У панели Exposure подменю нет, остальные инструменты не реализованы в v1.
        ToolId::Exposure | ToolId::Live | ToolId::Styles | ToolId::Night => Vec::new(),
    }
}

/// Выбранный сейчас вариант инструмента.
fn current_option(ctx: &Ctx<'_>, tool: ToolId) -> Option<ToolOption> {
    let settings = ctx.state.settings();
    match tool {
        ToolId::Flash => Some(ToolOption::Flash(settings.flash)),
        ToolId::Raw => Some(ToolOption::Raw(settings.raw)),
        ToolId::Timer => Some(ToolOption::Timer(settings.timer)),
        ToolId::Aspect => Some(ToolOption::Aspect(settings.aspect)),
        ToolId::Grid => Some(ToolOption::Grid(settings.grid)),
        ToolId::Level => Some(ToolOption::Level(settings.level)),
        ToolId::Exposure | ToolId::Live | ToolId::Styles | ToolId::Night => None,
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
        let mut options = Vec::with_capacity(MAX_SUBMENU_OPTIONS);
        for slot in 0..MAX_SUBMENU_OPTIONS {
            let rect = ctx.geometry.option_row(slot);
            let node = kit.add_node(sheet_node, "submenu_option", rect, sheet_rect)?;
            let button = PressButton::attach(kit, ButtonId::SubmenuOption(slot as u8), node, rect, ctx)?;
            kit.set_interactive(node, false)?;
            options.push(OptionSlot { button, option: None });
        }
        let back_rect = ctx.geometry.submenu_back();
        let back_node = kit.add_node(sheet_node, "submenu_back", back_rect, sheet_rect)?;
        let back = PressButton::attach(kit, ButtonId::SubmenuBack, back_node, back_rect, ctx)?;
        let back_content = kit.add_rotating_content(back_node, "submenu_back_icon")?;
        kit.set_interactive(back_node, false)?;
        kit.set_interactive(sheet_node, false)?;

        let spec = PanelMorphSpec::new(
            ctx.geometry.menu_button(),
            sheet_rect,
            ctx.geometry.config().sheet.corner_radius_pt,
        );
        let sheet_morph =
            PanelMorph::new(spec, &ctx.environment(sheet_rect.size.height), ctx.now).map_err(element("шторка меню"))?;
        Ok(Self {
            sheet_node,
            sheet_morph,
            sheet_open_wanted: false,
            items,
            options,
            back,
            back_content,
            submenu: None,
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
            ScreenGesture::Press(ButtonId::SubmenuOption(slot)) => {
                let option_slot = self
                    .options
                    .get_mut(usize::from(slot))
                    .ok_or_else(|| WidgetError::Inconsistent(format!("варианта подменю {slot} нет")))?;
                if option_slot.button.handle(event.state, event.data, event.time)? == ButtonEvent::Activated
                    && let Some(option) = option_slot.option
                {
                    if let Some(submenu) = &mut self.submenu {
                        submenu.picker.select_value(option).map_err(element("выбор варианта"))?;
                    }
                    out.actions.push(ScreenAction::SubmenuOptionSelected(option));
                }
            }
            ScreenGesture::Press(ButtonId::SubmenuBack) => {
                if self.back.handle(event.state, event.data, event.time)? == ButtonEvent::Activated {
                    out.actions.push(ScreenAction::SubmenuBackRequested);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Прозрачность кнопки меню (источника морфа): растворяется, пока из неё растёт шторка.
    pub(crate) fn source_opacity(&self, now: ui_core::time::MonotonicNanos) -> f32 {
        self.sheet_morph.snapshot(now).source_opacity
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let panel = ctx.state.panel();
        let sheet_open = matches!(panel, PanelState::ControlsMenu | PanelState::Submenu(_));
        if sheet_open != self.sheet_open_wanted {
            self.sheet_open_wanted = sheet_open;
            if sheet_open {
                self.sheet_morph.open(ctx.now).map_err(element("шторка меню"))?;
            } else {
                self.sheet_morph.close(ctx.now).map_err(element("шторка меню"))?;
            }
            kit.set_interactive(self.sheet_node, sheet_open)?;
        }
        let menu_level = panel == PanelState::ControlsMenu;
        for item in &self.items {
            kit.set_interactive(item.button.node(), menu_level && item.tool.is_some())?;
        }
        self.sync_submenu(kit, ctx, panel)
    }

    /// Открывает/закрывает подменю по состоянию экрана.
    fn sync_submenu(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>, panel: PanelState) -> Result<(), WidgetError> {
        match panel {
            PanelState::Submenu(tool) => {
                if self.submenu.as_ref().map(|submenu| submenu.tool) != Some(tool) {
                    self.open_submenu(kit, ctx, tool)?;
                } else {
                    self.reenter_submenu(kit, ctx, tool)?;
                }
            }
            PanelState::Closed | PanelState::ControlsMenu | PanelState::ExposurePanel => {
                if let Some(submenu) = &mut self.submenu
                    && submenu.morph.phase(ctx.now) != PanelPhase::Closing
                    && submenu.morph.phase(ctx.now) != PanelPhase::Closed
                {
                    submenu.morph.close(ctx.now).map_err(element("подменю"))?;
                }
                for slot in &mut self.options {
                    kit.set_interactive(slot.button.node(), false)?;
                    slot.option = None;
                }
                kit.set_interactive(self.back.node(), false)?;
            }
        }
        Ok(())
    }

    /// Подменю `tool` уже есть. Если оно закрывается (пользователь вышел и тут же вошёл снова), морф
    /// разворачивается (пружина продолжается с текущего места), а варианты и кнопка «назад» снова
    /// получают касания сразу, а не после конца морфа. Подсветка выбранного следует за состоянием экрана.
    fn reenter_submenu(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>, tool: ToolId) -> Result<(), WidgetError> {
        let Some(submenu) = &mut self.submenu else {
            return Ok(());
        };
        let was_closing = matches!(submenu.morph.phase(ctx.now), PanelPhase::Closing | PanelPhase::Closed);
        if was_closing {
            submenu.morph.open(ctx.now).map_err(element("подменю"))?;
        }
        if let Some(current) = current_option(ctx, tool) {
            // Состояние — владелец выбора: подсветка варианта следует за ним.
            submenu
                .picker
                .select_value(current)
                .map_err(element("выбор варианта"))?;
        }
        if was_closing {
            let entries = options_for(ctx, tool);
            self.bind_option_slots(kit, &entries)?;
            kit.set_interactive(self.back.node(), true)?;
        }
        Ok(())
    }

    /// Раздаёт варианты строкам подменю: занятые строки принимают касания, остальные нет.
    fn bind_option_slots(&mut self, kit: &mut SceneKit, entries: &[(ToolOption, String)]) -> Result<(), WidgetError> {
        for (slot, entry) in self
            .options
            .iter_mut()
            .zip(entries.iter().map(Some).chain(std::iter::repeat(None)))
        {
            slot.option = entry.map(|(option, _)| *option);
            kit.set_interactive(slot.button.node(), entry.is_some())?;
        }
        Ok(())
    }

    fn open_submenu(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>, tool: ToolId) -> Result<(), WidgetError> {
        let entries = options_for(ctx, tool);
        if entries.len() > self.options.len() {
            return Err(WidgetError::Inconsistent(format!(
                "у {tool:?} вариантов {}, строк подменю {}",
                entries.len(),
                self.options.len()
            )));
        }
        let current = current_option(ctx, tool)
            .or_else(|| entries.first().map(|(option, _)| *option))
            .ok_or_else(|| WidgetError::Inconsistent(format!("у {tool:?} нет подменю")))?;
        let picker_options = entries
            .iter()
            .enumerate()
            .map(|(index, (option, _))| PickerOption {
                value: *option,
                label: LabelId(index as u32),
                icon: OptionIcon::LabelOnly,
            })
            .collect();
        let picker = OptionPicker::new(picker_options, current).map_err(element("выбор варианта"))?;
        // Морф растёт из круга пункта (если инструмент есть в сетке) или из кнопки меню.
        let source: Rect = self
            .items
            .iter()
            .position(|item| item.tool == Some(tool))
            .map_or_else(|| ctx.geometry.menu_button(), |slot| ctx.geometry.menu_item(slot));
        let spec = PanelMorphSpec::new(
            source,
            ctx.geometry.sheet(),
            ctx.geometry.config().sheet.corner_radius_pt,
        )
        .with_spring(MotionToken::Smooth);
        let mut morph = PanelMorph::new(spec, &ctx.environment(ctx.geometry.sheet().size.height), ctx.now)
            .map_err(element("подменю"))?;
        morph.open(ctx.now).map_err(element("подменю"))?;
        self.bind_option_slots(kit, &entries)?;
        kit.set_interactive(self.back.node(), true)?;
        self.submenu = Some(Submenu {
            tool,
            morph,
            picker,
            labels: entries.into_iter().map(|(_, label)| label).collect(),
        });
        Ok(())
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>) {
        self.sheet_morph.advance(ctx.now);
        let _sheet_events = self.sheet_morph.drain_events();
        let mut finished = false;
        if let Some(submenu) = &mut self.submenu {
            submenu.morph.advance(ctx.now);
            let _events = submenu.morph.drain_events();
            finished = submenu.morph.phase(ctx.now) == PanelPhase::Closed;
        }
        if finished {
            self.submenu = None;
        }
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let sheet = self.sheet_morph.snapshot(ctx.now);
        if sheet.phase == PanelPhase::Closed {
            return Ok(());
        }
        let submenu_snapshot = self.submenu.as_ref().map(|submenu| submenu.morph.snapshot(ctx.now));
        let submenu_progress = submenu_snapshot.map_or(0.0, |snapshot| snapshot.progress as f32);
        // Стекло не на стекле: пока подменю закрывает шторку, её собственное стекло гаснет.
        let sheet_glass = PanelSnapshot {
            effect: sheet.effect * (1.0 - submenu_progress).clamp(0.0, 1.0),
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
        let menu_alpha = sheet.content_opacity * (1.0 - submenu_progress).clamp(0.0, 1.0);
        self.draw_items(
            ctx,
            visuals,
            layers,
            texts,
            menu_alpha,
            ctx.px.px(sheet.content_offset_pt),
        )?;
        if let (Some(submenu), Some(snapshot)) = (&self.submenu, submenu_snapshot)
            && snapshot.phase != PanelPhase::Closed
        {
            layers.push(
                Layer::Glass,
                DrawItem::Glass(panel_glass_group(
                    &snapshot,
                    ctx.tokens.glass.style_for(GlassRole::Panel),
                    ctx.px,
                )?),
            );
            self.draw_submenu(ctx, visuals, layers, texts, submenu, &snapshot)?;
        }
        Ok(())
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
        let sheet_config = &ctx.geometry.config().sheet;
        let colors = &ctx.tokens.colors;
        for item in &self.items {
            let (Some(tool), Some(button), Some(content)) = (
                item.tool,
                visuals.placement(item.button.node()),
                visuals.placement(item.content),
            ) else {
                continue;
            };
            let item_alpha = (button.opacity * alpha).clamp(0.0, 1.0);
            let opacity = Opacity::new(item_alpha)
                .map_err(|_| WidgetError::Inconsistent("прозрачность пункта вне 0..1".to_owned()))?;
            let active = tool_is_active(ctx, tool);
            let fill = DisplayColor {
                a: sheet_config.item_fill_alpha * if active { 2.0 } else { 1.0 },
                ..if active {
                    colors.focus_yellow
                } else {
                    colors.text_primary
                }
            };
            let slide = Affine2::translate(0.0, slide_px);
            layers.push(
                Layer::Content,
                DrawItem::Shape(ShapeItem {
                    geometry: ShapeGeometry::Circle {
                        diameter: button.size.width.min(button.size.height),
                    },
                    transform: slide.then(button.transform),
                    fill,
                    stroke: None,
                    shadow: None,
                    opacity,
                }),
            );
            let icon_color = if active {
                colors.focus_yellow
            } else {
                colors.text_primary
            };
            let content_host = slide.then(content.transform);
            let settings = ctx.state.settings();
            let icon = match tool {
                ToolId::Flash => Some(if settings.flash == FlashMode::On {
                    IconId::FlashOn
                } else {
                    IconId::FlashOff
                }),
                ToolId::Timer => Some(IconId::Timer),
                ToolId::Exposure => Some(IconId::Exposure),
                ToolId::Aspect => Some(IconId::AspectRatio),
                ToolId::Grid => Some(IconId::Grid),
                ToolId::Level => Some(IconId::Level),
                // RAW у iOS остаётся подписью внутри круга, не иконкой.
                ToolId::Raw | ToolId::Live | ToolId::Styles | ToolId::Night => None,
            };
            match icon {
                Some(icon) => layers.push(
                    Layer::Content,
                    centered_icon(
                        icon,
                        sheet_config.item_icon_size_pt,
                        ctx.px,
                        content_host,
                        icon_color,
                        opacity,
                    ),
                ),
                None => {
                    let run = texts.run(ctx, tool_caption(tool), TextRoleKind::Caption)?;
                    layers.push(Layer::Content, centered_text(run, content_host, icon_color, opacity));
                }
            }
            // Подпись под кругом не поворачивается: она ставится от узла пункта, а не узла значка.
            let caption = texts.run(ctx, tool_caption(tool), TextRoleKind::MenuItem)?;
            let caption_shift = Affine2::translate(
                0.0,
                button.size.height * 0.5 + ctx.px.px(sheet_config.caption_below_circle_pt),
            );
            layers.push(
                Layer::Content,
                centered_text(
                    caption,
                    caption_shift.then(slide).then(button.transform),
                    colors.text_primary,
                    opacity,
                ),
            );
        }
        Ok(())
    }

    fn draw_submenu(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        submenu: &Submenu,
        snapshot: &PanelSnapshot,
    ) -> Result<(), WidgetError> {
        let alpha = snapshot.content_opacity.clamp(0.0, 1.0);
        let slide_px = ctx.px.px(snapshot.content_offset_pt);
        let colors = &ctx.tokens.colors;
        // Кнопка «назад»: стрелка вверх, повёрнутая на 90° против часовой, смотрит влево.
        if let (Some(button), Some(content)) = (
            visuals.placement(self.back.node()),
            visuals.placement(self.back_content),
        ) {
            let content = NodePlacementPx {
                transform: Affine2::rotate(-std::f32::consts::FRAC_PI_2).then(content.transform),
                ..content
            };
            let sheet_config = &ctx.geometry.config().sheet;
            let size = sheet_config.back_diameter_pt * sheet_config.back_icon_scale;
            draw_plain_icon_button(
                ctx,
                layers,
                &button,
                &content,
                IconId::MenuChevron,
                size,
                alpha,
                slide_px,
            )?;
        }
        let slide = Affine2::translate(0.0, slide_px);
        for (index, label) in submenu.labels.iter().enumerate() {
            let Some(slot) = self.options.get(index) else { continue };
            let Some(row) = visuals.placement(slot.button.node()) else {
                continue;
            };
            let selected = submenu.picker.selected_index() == index;
            let row_alpha = (row.opacity * alpha).clamp(0.0, 1.0);
            let opacity = Opacity::new(row_alpha)
                .map_err(|_| WidgetError::Inconsistent("прозрачность строки вне 0..1".to_owned()))?;
            // Выбранный вариант: тёмная подложка `scrim` + жёлтый текст. Жёлтая заливка под жёлтым текстом
            // сливалась на тёплом стекле (над оранжевым фоном), а тёмная капсула держит контраст на любом фоне.
            let fill = if selected {
                colors.scrim
            } else {
                DisplayColor {
                    a: ctx.geometry.config().sheet.item_fill_alpha,
                    ..colors.text_primary
                }
            };
            layers.push(
                Layer::Content,
                DrawItem::Shape(ShapeItem {
                    geometry: ShapeGeometry::Capsule { size: row.size },
                    transform: slide.then(row.transform),
                    fill,
                    stroke: None,
                    shadow: None,
                    opacity,
                }),
            );
            let run = texts.run(ctx, label, TextRoleKind::MenuItem)?;
            // Подпись прижата к левому краю строки с отступом в полвысоты (капсула).
            let left_pad = row.size.height * 0.5;
            let shift = Affine2::translate(-row.size.width * 0.5 + left_pad + run.advance_px * 0.5, 0.0);
            let color = if selected {
                colors.focus_yellow
            } else {
                colors.text_primary
            };
            layers.push(
                Layer::Content,
                centered_text(run, shift.then(slide).then(row.transform), color, opacity),
            );
        }
        Ok(())
    }
}
