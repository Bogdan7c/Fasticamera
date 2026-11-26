//! Колонка ручек слева (решения 185, 199, 203, 204): стеклянная панель, под ней видоискатель; сверху шапка (основа
//! тона и «чистяк»), под ней строка переключателя вида SCALES | WHEELS (не прокручивается, видна в обоих видах),
//! ниже окно вида: прокручиваемый список шкал или колёса цвета без прокрутки.
//!
//! Прокрутка списка — `ui_kit::scroll_list::ScrollList`: палец ведёт содержимое, узел содержимого сдвигается по
//! вертикали на смещение, а хит-тест `ui-core` сам учитывает сдвиг. Узел окна списка ограничивает касания его
//! рамкой (дети за рамкой родителя не ловят), поэтому строки, уехавшие под шапку или за низ панели, нажать нельзя.
//! Рисование вида ограничено той же рамкой окна: строка у края режется ровно по нему (`DrawLayers::clipped_to`, решение 227).

use look_contract::BandFacts;
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, GestureState, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::draw_list::PressState;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_kit::drive::MotionPhase;
use ui_kit::pan::PanUpdate;
use ui_kit::scroll_list::{ScrollAxis, ScrollList};
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::GlassShape;

use super::control_view::{ControlView, RowsView, VisibleWindow};
use super::finger_focus::{DrivenControl, FingerFocus};
use super::knob_row::WHEEL_RING_SLOT;
use super::segments::{SegmentRow, SegmentSpec, push_if_activated};
use super::space::{EyeAxis, SectionSpace};
use super::tone_header::ToneHeader;
use super::wheels::WheelsView;
use crate::action::ScreenAction;
use crate::calibration::{ColumnFrames, ControlViewKind, WheelsFrames};
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs, SECTION_PANEL_MATERIAL, single_member_material};

/// Сегменты переключателя вида в порядке показа: SCALES | WHEELS (тексты UI английские).
const VIEW_SEGMENTS: [(ControlViewKind, &str); 2] =
    [(ControlViewKind::Scales, "SCALES"), (ControlViewKind::Wheels, "WHEELS")];

/// Колонка ручек.
pub(super) struct KnobsColumn {
    panel: NodeId,
    header: ToneHeader,
    /// Переключатель вида SCALES | WHEELS: под шапкой, вне окна прокрутки.
    switcher: SegmentRow<ControlViewKind>,
    /// Окно вида: его рамка ограничивает и касания, и рисование содержимого.
    window: NodeId,
    content_node: NodeId,
    list: ScrollList,
    view: ControlView,
    /// Высота окна списка, pt.
    window_height_pt: f64,
    /// Смещение списка, которое состояние раздела уже знает (`ScalesScrollRemembered`): как только список
    /// успокоился на другом смещении, оно уходит состоянию, чтобы возврат на SCALES и пересборка раздела не
    /// сбрасывали список на верх.
    remembered_offset_pt: f64,
    /// «Картинка под пальцем» (решение 273): переходное состояние жеста, не часть `ScreenState`.
    focus: FingerFocus,
}

/// Какой орган колонки ведёт жест с таким намерением; `None` — жест не из органов, которые прячут остальное.
/// Шкала кольца яркости — часть блока колеса, а не «строка ручки».
fn driven_control_of(intent: ScreenGesture) -> Option<DrivenControl> {
    match intent {
        ScreenGesture::CalibrationRulerPan(WHEEL_RING_SLOT) | ScreenGesture::CalibrationWheelPadPan => {
            Some(DrivenControl::Wheels)
        }
        ScreenGesture::CalibrationRulerPan(slot) => Some(DrivenControl::Row(slot)),
        _ => None,
    }
}

impl KnobsColumn {
    /// Строит колонку в `parent`; `panel_rect` — рамка стеклянной панели в системе корня раздела.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (parent, parent_rect): (NodeId, Rect),
        (panel_rect, bands, view_kind): (Rect, &[BandFacts], ControlViewKind),
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let panel = kit.add_node(parent, "calib_knobs_panel", panel_rect, parent_rect)?;
        // Рамки колонки — общая функция `calibration::ColumnFrames`: те же числа читает запрос геометрии фасада.
        let frames = ColumnFrames::of(panel_rect, config);
        let header = ToneHeader::build(kit, ctx, space, (panel, panel_rect), frames.header)?;
        let switcher = SegmentRow::build(
            kit,
            ctx,
            (panel, panel_rect),
            frames.switcher,
            SegmentSpec {
                entries: &VIEW_SEGMENTS,
                node_label: "calib_view_segment",
                id_of: ButtonId::CalibrationControlView,
                text_role: TextRoleKind::SegmentLabel,
            },
        )?;
        // Окно вида — вся ширина панели под переключателем; содержимое уже на отступы и едет внутри окна.
        let window_rect = frames.window;
        let list_node = kit.add_node(panel, "calib_knobs_window", window_rect, panel_rect)?;
        kit.add_recognizer(
            list_node,
            RecognizerSpec::Pan(PanSpec {
                direction: space.pan_direction(EyeAxis::Vertical),
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationListPan,
        )?;
        let (view_rect, view_is_scrollable) = match view_kind {
            ControlViewKind::Scales => (
                Rect::from_xywh(
                    frames.content.min_x(),
                    window_rect.min_y(),
                    frames.content.size.width,
                    RowsView::planned_height(config),
                ),
                true,
            ),
            ControlViewKind::Wheels => (frames.content, false),
        };
        let content_node = kit.add_node(list_node, "calib_knobs_content", view_rect, window_rect)?;
        let view = match view_kind {
            ControlViewKind::Scales => {
                ControlView::Rows(RowsView::build(kit, ctx, space, (content_node, view_rect), bands)?)
            }
            ControlViewKind::Wheels => {
                let pixels_per_point = ctx.state.pixels_per_point().ok_or_else(|| {
                    WidgetError::Inconsistent("вид колёс строится без размера поверхности".to_owned())
                })?;
                // Не поместился круг — не ошибка сборки: колонка живёт без колёс, причину считает редьюсер.
                match WheelsFrames::of(view_rect, config, pixels_per_point) {
                    Ok(wheel_frames) => ControlView::Wheels(Box::new(WheelsView::build(
                        kit,
                        ctx,
                        space,
                        (content_node, view_rect),
                        wheel_frames,
                    )?)),
                    Err(_) => ControlView::Unavailable {
                        window_height_pt: view_rect.size.height,
                    },
                }
            }
        };
        debug_assert!(
            view_is_scrollable || view.content_height() <= window_rect.size.height + 1e-9,
            "вид без прокрутки обязан помещаться в окно"
        );
        // Список шкал начинает с запомненного смещения (его зажмёт в диапазон: окно может быть ниже или выше, чем
        // было); у вида колёс прокрутки нет — он стоит на нуле.
        let initial_offset_pt = match view_kind {
            ControlViewKind::Scales => ctx.state.calibration().scales_scroll_offset_pt(),
            ControlViewKind::Wheels => 0.0,
        };
        let list = ScrollList::along(
            ScrollAxis::Vertical,
            view.content_height(),
            initial_offset_pt,
            &ctx.environment(window_rect.size.height),
            ctx.now,
        )
        .map_err(element("список ручек"))?;
        Ok(Self {
            panel,
            header,
            switcher,
            window: list_node,
            content_node,
            view,
            window_height_pt: window_rect.size.height,
            remembered_offset_pt: list.offset(ctx.now),
            focus: FingerFocus::default(),
            list,
        })
    }

    /// Смещение прокрутки списка, pt (проба для тестов).
    #[cfg(test)]
    pub(super) fn scroll_offset(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.list.offset(now)
    }

    /// Положение линейки строки `slot` в шагах (проба для тестов).
    #[cfg(test)]
    pub(super) fn row_position_steps(&self, slot: u8, now: ui_core::time::MonotonicNanos) -> Option<f64> {
        self.view.row_position_steps(slot, now)
    }

    pub(super) fn invalid_values(&self) -> u64 {
        self.view.invalid_values()
    }

    pub(super) fn suppressed_echo_events(&self) -> u64 {
        self.view.suppressed_echo_events()
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent == ScreenGesture::CalibrationListPan {
            if let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан списка"))?
            {
                self.list
                    .apply_pan(event.time, space.eye_pan(update))
                    .map_err(element("список ручек"))?;
            }
            return Ok(true);
        }
        self.track_finger_focus(event);
        if self.header.handle_gesture(kit, space, ctx, event, out)? {
            return Ok(true);
        }
        let switch_event = self.switcher.handle_gesture(kit, event)?;
        if push_if_activated(switch_event, out, ScreenAction::ControlViewSelected) {
            return Ok(true);
        }
        self.view.handle_gesture(kit, space, ctx, event, out)
    }

    /// Следит за тем, ведёт ли палец ручку или колесо: на `Began` орган становится ведомым (остальное
    /// растворится в `sync`), на конце жеста возвращается.
    fn track_finger_focus(&mut self, event: &GestureEvent<ScreenGesture>) {
        let Some(driven) = driven_control_of(event.intent) else {
            return;
        };
        match event.state {
            GestureState::Began => self.focus.begin(driven, self.view.focus_nodes(driven)),
            GestureState::Ended | GestureState::Cancelled | GestureState::Failed => self.focus.end(driven),
            GestureState::Possible | GestureState::Changed => {}
        }
    }

    /// Во сколько раз «остальное» прозрачнее покоя сейчас: 1 в покое, `finger_focus_others_opacity` под пальцем.
    /// Меряется по датчику (растворённому корню) относительно панели: прозрачность узлов перемножается с предками.
    fn others_opacity(&self, visuals: &VisualIndex) -> f32 {
        let Some(probe) = self.focus.probe() else {
            return 1.0;
        };
        let (Some(probe), Some(panel)) = (visuals.placement(probe), visuals.placement(self.panel)) else {
            return 1.0;
        };
        if panel.opacity <= 0.0 {
            return 1.0;
        }
        (probe.opacity / panel.opacity).clamp(0.0, 1.0)
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        // Растворённым узлам возвращается хит-тест ДО синхронизации строк: они сами выставят своё настоящее состояние.
        self.focus.restore_interactivity(kit)?;
        // Содержимое едет за списком: смещение вверх — узел вниз по системе корня с минусом.
        kit.jump_vertically_to(self.content_node, -self.list.offset(ctx.now))?;
        self.header.sync(kit, ctx)?;
        // Переключатель вида работает и без фактов: выбор вида — состояние экрана, ему факты не нужны (а без него из
        // вида колёс, который не нужен без библиотеки пресетов, нельзя было бы выйти). Вкладки же и круг ведут
        // значения вида и без фактов молчат.
        self.switcher.set_touchable(kit, true)?;
        self.view.sync(kit, ctx)?;
        // После всех синхронизаций: остальное растворяется (или возвращается) и не ловит новых касаний.
        let others_opacity = f64::from(ctx.config.calibration.widgets.finger_focus_others_opacity);
        self.focus.reconcile(kit, ctx, self.panel, others_opacity)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.header.advance(out);
        self.view.advance(ctx, out);
        self.remember_scroll_when_settled(ctx, out);
    }

    /// Когда список шкал успокоился на смещении, которого состояние раздела ещё не знает, отдаёт ему это смещение.
    /// Сравнение смещений, а не «было движение»: за один кадр палец может провести и отпустить список, и фазы
    /// движения кадр не увидит. Вид колёс не прокручивается и ничего не запоминает (его нулевое смещение затёрло
    /// бы запомненное у шкал).
    fn remember_scroll_when_settled(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        if !matches!(self.view, ControlView::Rows(_)) || self.list.phase(ctx.now) != MotionPhase::Resting {
            return;
        }
        let offset_pt = self.list.offset(ctx.now);
        if offset_pt != self.remembered_offset_pt {
            self.remembered_offset_pt = offset_pt;
            out.actions.push(ScreenAction::ScalesScrollRemembered { offset_pt });
        }
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.panel) else {
            return Ok(());
        };
        let others_opacity = self.others_opacity(visuals);
        // Панель — предок ведомого органа, сцена её не растворяет: множитель накладывается здесь.
        if others_opacity > 0.0 {
            let (item, _host) = single_member_material(
                ctx,
                SECTION_PANEL_MATERIAL,
                GlassShape::RoundedRect {
                    corner_radius_pt: ctx.config.calibration.widgets.panel_corner_radius_pt,
                },
                &NodePlacementPx {
                    opacity: placement.opacity * others_opacity,
                    ..placement
                },
                PressState::RELEASED,
                "calib_knobs_panel",
            )?;
            layers.push(Layer::Glass, item);
        }
        self.draw_finger_backing(ctx, visuals, layers, others_opacity)?;
        self.header.draw(ctx, visuals, layers)?;
        self.switcher.draw(
            ctx,
            (visuals, texts),
            layers,
            (ctx.state.calibration().control_view(), |_| false, 1.0),
        )?;
        let Some(window_rect) = visuals
            .placement(self.window)
            .and_then(|window| window.screen_rect_px())
        else {
            return Ok(());
        };
        layers.clipped_to(window_rect, |layers| {
            self.view.draw(
                ctx,
                visuals,
                layers,
                texts,
                VisibleWindow {
                    offset_pt: self.list.offset(ctx.now),
                    height_pt: self.window_height_pt,
                    others_opacity,
                },
            )
        })
    }

    /// Подложка под ведомым органом: та же поверхность раздела без стекла, чтобы орган читался прямо над сценой,
    /// когда панель растворилась. Проявляется настолько, насколько растворилось остальное (в покое её нет).
    fn draw_finger_backing(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        others_opacity: f32,
    ) -> Result<(), WidgetError> {
        let backing_opacity = 1.0 - others_opacity;
        if backing_opacity <= 0.0 {
            return Ok(());
        }
        let config = &ctx.config.calibration.widgets;
        let padding_px = ctx.px.px(config.finger_focus_backing_padding_pt) * 2.0;
        for node in self.focus.driven_nodes() {
            let Some(placement) = visuals.placement(*node) else {
                continue;
            };
            let inflated = NodePlacementPx {
                size: PxSize::new(placement.size.width + padding_px, placement.size.height + padding_px),
                opacity: placement.opacity * backing_opacity,
                ..placement
            };
            let (item, _host) = single_member_material(
                ctx,
                SECTION_PANEL_MATERIAL,
                GlassShape::RoundedRect {
                    corner_radius_pt: config.finger_focus_backing_corner_radius_pt,
                },
                &inflated,
                PressState::RELEASED,
                "calib_finger_backing",
            )?;
            layers.push(Layer::Glass, item);
        }
        Ok(())
    }
}
