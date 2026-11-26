//! Карточки меню раздела калибровки: PRESETS и SCOPE AXES (решения 94, 199, 201).
//!
//! Та же нижняя карточка, что у Aspect и Timer в PHOTO (`tool_card.rs`): стеклянная рамка вырастает из шторки
//! (`PanelMorph`), заголовок ЗАГЛАВНЫМИ, боковые кнопки «‹» и «↻» (общий `SideButton`), ряд таблеток (общий
//! `ChipRow`). Размер, радиус, расстановка и пружины — те же числа `widgets.card` (измерены), потому что карточка
//! здесь не «похожая», а та же; различие данные:
//! * PRESETS — вместо крупного значения ряд действий (NEW / RENAME / SAVE / DELETE) на высоте значения, ниже ряд имён
//!   пресетов; «↻» — вернуть правки к пресету;
//! * SCOPE AXES — крупное значение (текущий стандарт осей) и ряд из двух стандартов; «↻» нет.
//!
//! # Инварианты
//! * Какой карточкой владеет меню, решает `ScreenState` (`calibration().menu()`); виджет только выравнивается по нему.
//! * Пока карточка закрывается, состояние уже `Closed`: вид карточки запоминается в `shown`, чтобы содержимое
//!   доисчезало тем же, чем было; касания снимаются сразу.
//! * Что значит нажатие, решает карточка и отдаёт действием; ряды ничего не знают о пресетах.

use crate::widget::SECTION_PANEL_MATERIAL;
use look_contract::{EditState, PresetId, UserPresetId};
use scope_contract::VectorscopeAxes;
use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::color::Opacity;
use ui_draw::icons::IconId;
use ui_draw::transform::Affine2;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase};
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{centered_text, panel_material_hosted};

use crate::action::ScreenAction;
use crate::calibration::{CalibrationMenu, DeleteConfirmation};
use crate::state::ScreenState;
use crate::widget::calibration::host::RootHost;
use crate::widget::calibration::space::{EyeAxis, SectionSpace};
use crate::widget::chip_row::{ChipFace, ChipRow, ChipRowIds, ChipTone, RowEvent, RowPaint};
use crate::widget::error::{WidgetError, element};
use crate::widget::geometry::ScreenGeometry;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::side_button::{IconTurn, SideButton, SideButtonLook};
use crate::widget::text_runs::TextRuns;
use crate::widget::tool_card::MENU_MORPH_MOTION;
use crate::widget::tool_card_styles::{preset_chip_faces, preset_ids, styles_have_edits};
use crate::widget::{Ctx, Outputs};

/// Стандарты осей вектороскопа в порядке ряда и их подписи (английский UI). `[invented]`.
const AXES_OPTIONS: [(VectorscopeAxes, &str); 2] = [
    (VectorscopeAxes::YCbCrBt709, "Y'CbCr 709"),
    (VectorscopeAxes::OklabAb, "OKLAB"),
];

/// Заголовок и подпись карточки по её виду. `[invented]`.
fn title_of(kind: CardKind) -> &'static str {
    match kind {
        CardKind::Presets => "PRESETS",
        CardKind::ScopeAxes => "SCOPE AXES",
    }
}

fn axes_label(axes: VectorscopeAxes) -> &'static str {
    AXES_OPTIONS
        .iter()
        .find(|(candidate, _)| *candidate == axes)
        .map_or("", |(_, label)| label)
}

/// Какая из двух карточек показана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardKind {
    Presets,
    ScopeAxes,
}

/// Что делает таблетка верхнего ряда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopTarget {
    Preset(PresetId),
    Axes(VectorscopeAxes),
}

/// Таблетки ряда действий пресета (NEW / RENAME / SAVE / DELETE) и действие каждой; `None` — таблетка недоступна.
/// Правила: NEW — при любых фактах; RENAME, SAVE, DELETE — только у своего пресета; SAVE ещё и только при правках;
/// DELETE после первого тапа показывает «CONFIRM» (двойное подтверждение, `DeleteConfirmation`).
pub(super) fn preset_actions(state: &ScreenState) -> Vec<(ChipFace, Option<ScreenAction>)> {
    let Some(facts) = state.look_facts() else {
        return Vec::new();
    };
    let user_preset: Option<UserPresetId> = match facts.active {
        PresetId::User(id) => Some(id),
        PresetId::Builtin(_) => None,
    };
    let gate = |action: Option<ScreenAction>| -> (ChipTone, Option<ScreenAction>) {
        match action {
            Some(action) => (ChipTone::Plain, Some(action)),
            None => (ChipTone::Disabled, None),
        }
    };
    let (new_tone, new_action) = gate(Some(ScreenAction::CreatePresetTapped));
    let (rename_tone, rename_action) = gate(user_preset.map(ScreenAction::RenamePresetTapped));
    let (save_tone, save_action) = gate(
        user_preset
            .filter(|_| facts.edits == EditState::EditedOnTop)
            .map(|_| ScreenAction::OverwritePresetTapped),
    );
    let (delete_tone, delete_action) = gate(user_preset.map(ScreenAction::DeletePresetTapped));
    let armed =
        user_preset.is_some_and(|id| state.calibration().delete_confirmation() == DeleteConfirmation::Armed(id));
    let (delete_text, delete_tone) = if armed {
        ("CONFIRM", ChipTone::Highlighted)
    } else {
        ("DELETE", delete_tone)
    };
    vec![
        (ChipFace::new("NEW", new_tone), new_action),
        (ChipFace::new("RENAME", rename_tone), rename_action),
        (ChipFace::new("SAVE", save_tone), save_action),
        (ChipFace::new(delete_text, delete_tone), delete_action),
    ]
}

fn axes_faces(state: &ScreenState) -> Vec<ChipFace> {
    let current = state.calibration().vectorscope_axes();
    AXES_OPTIONS
        .iter()
        .map(|(axes, label)| {
            ChipFace::new(
                *label,
                if *axes == current {
                    ChipTone::Highlighted
                } else {
                    ChipTone::Plain
                },
            )
        })
        .collect()
}

pub(super) struct MenuCard {
    node: NodeId,
    left: SideButton,
    right: SideButton,
    /// Верхний ряд: имена пресетов или стандарты осей.
    top: ChipRow,
    /// Ряд действий пресета.
    actions: ChipRow,
    eye: ScreenGeometry,
    sheet_rect: Rect,
    card_rect: Rect,
    morph: Option<PanelMorph>,
    open_wanted: bool,
    /// Показанная карточка: нужна и во время закрытия, когда состояние уже забыло о ней.
    shown: Option<CardKind>,
    /// Что делает таблетка верхнего ряда (по номеру) на момент последней сверки.
    top_targets: Vec<TopTarget>,
    /// Действия таблеток ряда действий на момент последней сверки.
    action_targets: Vec<Option<ScreenAction>>,
}

impl MenuCard {
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (eye, space): (&ScreenGeometry, &SectionSpace),
        (root, root_frame): (NodeId, Rect),
        sheet_rect: Rect,
    ) -> Result<Self, WidgetError> {
        let card_rect = eye.card();
        let node = kit.add_node(root, "calib_menu_card", card_rect, root_frame)?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Vertical),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationMenuClosePan,
        )?;
        // Ряды стоят там же, где в PHOTO: верхний — на месте ряда вариантов, действия — на высоте крупного значения.
        let inset = ctx.config.calibration.widgets.chip_padding_pt;
        let (top_center, row_width) = eye.card_chip_row(inset);
        let actions_center = Point::new(top_center.x, eye.card_value_center().y);
        let top = ChipRow::build(
            kit,
            ctx,
            (node, card_rect),
            (top_center, row_width),
            (
                ChipRowIds {
                    chip: ButtonId::CalibrationChip,
                    pan: Some(ScreenGesture::CalibrationChipsPan),
                    chip_label: "calib_chip",
                },
                Some(*space),
            ),
        )?;
        let actions = ChipRow::build(
            kit,
            ctx,
            (node, card_rect),
            (actions_center, row_width),
            (
                ChipRowIds {
                    chip: ButtonId::CalibrationAction,
                    pan: None,
                    chip_label: "calib_action_chip",
                },
                Some(*space),
            ),
        )?;
        let left = SideButton::build(
            kit,
            ctx,
            (node, card_rect),
            (ButtonId::CalibrationCardLeft, eye.card_left_button()),
            IconTurn::Fixed,
        )?;
        let right = SideButton::build(
            kit,
            ctx,
            (node, card_rect),
            (ButtonId::CalibrationCardRight, eye.card_right_button()),
            IconTurn::Fixed,
        )?;
        kit.set_interactive(node, false)?;
        Ok(Self {
            node,
            left,
            right,
            top,
            actions,
            eye: eye.clone(),
            sheet_rect,
            card_rect,
            morph: None,
            open_wanted: false,
            shown: None,
            top_targets: Vec::new(),
            action_targets: Vec::new(),
        })
    }

    /// Рамка карточки в системе корня.
    pub(super) fn rect(&self) -> Rect {
        self.card_rect
    }

    /// Насколько карточка закрыла шторку, 0..1: шторка гасит стекло и пункты на эту долю.
    pub(super) fn cover_progress(&self, now: MonotonicNanos) -> f32 {
        self.morph
            .as_ref()
            .map_or(0.0, |morph| (morph.snapshot(now).progress as f32).clamp(0.0, 1.0))
    }

    /// Имена пресетов, не поместившиеся в верхний ряд (диагностика).
    pub(super) fn overflowed_chips(&self) -> usize {
        self.top.overflowed_chips()
    }

    fn new_morph(&self, ctx: &Ctx<'_>) -> Result<PanelMorph, WidgetError> {
        let card_config = &ctx.geometry.config().card;
        let spec = PanelMorphSpec::new(self.sheet_rect, self.card_rect, card_config.corner_radius_pt)
            .with_source_corner_radius_pt(ctx.geometry.config().sheet.corner_radius_pt)
            .with_spring(MENU_MORPH_MOTION);
        PanelMorph::new(spec, &ctx.environment(self.card_rect.size.height), ctx.now)
            .map_err(element("карточка меню раздела"))
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wanted = match ctx.state.calibration().menu() {
            CalibrationMenu::Presets => Some(CardKind::Presets),
            CalibrationMenu::ScopeAxes => Some(CardKind::ScopeAxes),
            CalibrationMenu::Closed | CalibrationMenu::Root => None,
        };
        let Some(kind) = wanted else {
            if self.open_wanted {
                self.open_wanted = false;
                if let Some(morph) = &mut self.morph {
                    morph.close(ctx.now).map_err(element("карточка меню раздела"))?;
                }
            }
            return self.make_untouchable(kit);
        };
        if !self.open_wanted {
            self.open_wanted = true;
            // Идущий морф переиспользуем (открытие посреди закрытия — разворот с сохранением скорости).
            let reusable = self
                .morph
                .as_ref()
                .is_some_and(|morph| morph.phase(ctx.now) != PanelPhase::Closed);
            if !reusable {
                self.morph = Some(self.new_morph(ctx)?);
            }
            if let Some(morph) = &mut self.morph {
                morph.open(ctx.now).map_err(element("карточка меню раздела"))?;
            }
        }
        self.shown = Some(kind);
        kit.set_interactive(self.node, true)?;
        kit.set_interactive(self.left.button.node(), true)?;
        let presets = kind == CardKind::Presets;
        kit.set_interactive(self.right.button.node(), presets && styles_have_edits(ctx.state))?;
        match kind {
            CardKind::Presets => {
                self.top_targets = preset_ids(ctx.state).into_iter().map(TopTarget::Preset).collect();
                self.top.set_faces(kit, ctx, preset_chip_faces(ctx.state))?;
                let (faces, targets): (Vec<ChipFace>, Vec<Option<ScreenAction>>) =
                    preset_actions(ctx.state).into_iter().unzip();
                self.action_targets = targets;
                self.actions.set_faces(kit, ctx, faces)?;
            }
            CardKind::ScopeAxes => {
                self.top_targets = AXES_OPTIONS.iter().map(|(axes, _)| TopTarget::Axes(*axes)).collect();
                self.top.set_faces(kit, ctx, axes_faces(ctx.state))?;
                self.action_targets.clear();
                self.actions.set_faces(kit, ctx, Vec::new())?;
            }
        }
        self.top.set_touchable(kit, true)?;
        self.actions.set_touchable(kit, true)?;
        self.top.sync(kit, ctx)?;
        self.actions.sync(kit, ctx)
    }

    /// Карточка закрыта или закрывается: касания проходят мимо, содержимое дорисовывается до конца растворения.
    fn make_untouchable(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        kit.set_interactive(self.node, false)?;
        kit.set_interactive(self.left.button.node(), false)?;
        kit.set_interactive(self.right.button.node(), false)?;
        self.top.stop_touches(kit)?;
        self.actions.stop_touches(kit)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>) {
        if let Some(morph) = &mut self.morph {
            morph.advance(ctx.now);
            let _events = morph.drain_events();
            if !self.open_wanted && morph.phase(ctx.now) == PanelPhase::Closed {
                // Закрылась целиком: морф и запомненное содержимое больше не нужны.
                self.morph = None;
                self.shown = None;
                self.top.forget();
                self.actions.forget();
                self.top_targets.clear();
                self.action_targets.clear();
            }
        }
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::Press(ButtonId::CalibrationCardLeft) => {
                let data = kit.gesture_data_in_node(self.left.button.node(), event.data);
                if self.left.button.handle(event.state, data, event.time)?
                    == crate::widget::button::ButtonEvent::Activated
                {
                    out.actions.push(ScreenAction::SubmenuBackRequested);
                }
                Ok(true)
            }
            ScreenGesture::Press(ButtonId::CalibrationCardRight) => {
                let data = kit.gesture_data_in_node(self.right.button.node(), event.data);
                if self.right.button.handle(event.state, data, event.time)?
                    == crate::widget::button::ButtonEvent::Activated
                {
                    out.actions.push(ScreenAction::DiscardEditsTapped);
                }
                Ok(true)
            }
            _ => {
                match self.top.handle_gesture(kit, event)? {
                    RowEvent::NotMine => {}
                    RowEvent::Consumed => return Ok(true),
                    RowEvent::Activated(index) => {
                        match self.top_targets.get(index) {
                            Some(TopTarget::Preset(preset)) => out.actions.push(ScreenAction::PresetSelected(*preset)),
                            Some(TopTarget::Axes(axes)) => {
                                out.actions.push(ScreenAction::VectorscopeAxesSelected(*axes))
                            }
                            None => {}
                        }
                        return Ok(true);
                    }
                }
                match self.actions.handle_gesture(kit, event)? {
                    RowEvent::NotMine => Ok(false),
                    RowEvent::Consumed => Ok(true),
                    RowEvent::Activated(index) => {
                        if let Some(Some(action)) = self.action_targets.get(index) {
                            out.actions.push(action.clone());
                        }
                        Ok(true)
                    }
                }
            }
        }
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        (visuals, host): (&VisualIndex, &RootHost),
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let (Some(morph), Some(kind)) = (&self.morph, self.shown) else {
            return Ok(());
        };
        let snapshot = morph.snapshot(ctx.now);
        if snapshot.phase == PanelPhase::Closed {
            return Ok(());
        }
        layers.push(
            Layer::Glass,
            panel_material_hosted(&snapshot, ctx.tokens, SECTION_PANEL_MATERIAL, ctx.px, host.transform())?,
        );
        let alpha = snapshot.content_opacity.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Ok(());
        }
        let slide_px = ctx.px.px(snapshot.content_offset_pt);
        self.draw_texts(ctx, host, layers, texts, (kind, alpha, slide_px))?;
        let paint = RowPaint { alpha, slide_px };
        self.top.draw(ctx, visuals, layers, texts, paint)?;
        self.actions.draw(ctx, visuals, layers, texts, paint)?;
        self.draw_buttons(ctx, visuals, layers, (kind, alpha, slide_px))
    }

    /// Заголовок и (у SCOPE AXES) крупное значение по центру карточки.
    fn draw_texts(
        &self,
        ctx: &Ctx<'_>,
        host: &RootHost,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        (kind, alpha, slide_px): (CardKind, f32, f32),
    ) -> Result<(), WidgetError> {
        let opacity = Opacity::new(alpha)
            .map_err(|_| WidgetError::Inconsistent("прозрачность карточки меню раздела вне 0..1".to_owned()))?;
        let colors = &ctx.tokens.colors;
        let title = texts.run(ctx, title_of(kind), TextRoleKind::CardTitle)?;
        layers.push(
            Layer::Content,
            centered_text(
                title,
                host.point(ctx, self.eye.card_title_center(), slide_px),
                colors.text_muted,
                opacity,
            ),
        );
        if kind == CardKind::ScopeAxes {
            // Подписи стандартов — наши константы (ASCII), символов вне атласа в них нет.
            let label = axes_label(ctx.state.calibration().vectorscope_axes());
            let value = texts.run(ctx, label, TextRoleKind::CardValue)?;
            layers.push(
                Layer::Content,
                centered_text(
                    value,
                    host.point(ctx, self.eye.card_value_center(), slide_px),
                    colors.text_primary,
                    opacity,
                ),
            );
        }
        Ok(())
    }

    fn draw_buttons(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        (kind, alpha, slide_px): (CardKind, f32, f32),
    ) -> Result<(), WidgetError> {
        self.left.draw(
            ctx,
            visuals,
            layers,
            SideButtonLook {
                icon: IconId::MenuChevron,
                // «‹» — стрелка вверх, повёрнутая на 90° против часовой (смотрит влево), как в карточке PHOTO.
                rotation: Affine2::rotate(-std::f32::consts::FRAC_PI_2),
                content_alpha: alpha,
                slide_px,
                extra_opacity: 1.0,
            },
        )?;
        if kind == CardKind::Presets {
            let dim = if styles_have_edits(ctx.state) {
                1.0
            } else {
                ctx.geometry.config().card.reset_dim_opacity
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
}
