//! Сцена экрана под конкретный размер surface и масштаб: дерево, жесты и все виджеты.
//!
//! Сцена пересобирается при смене размера или масштаба (как в стенде стекла): положения пальцев и рамки
//! зависят от масштаба, поэтому старые жесты отбрасываются. Состояние экрана (`ScreenState`) при этом не
//! теряется, а элементы выравниваются по нему первой же синхронизацией.

use std::sync::Arc;

use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Insets;
use ui_core::gesture::{
    GestureData, GestureEvent, GestureState, LongPressSpec, PanDirection, PanSpec, PinchSpec, RecognizerSpec, TapSpec,
};
use ui_core::id::NodeId;
use ui_core::layout::AnchorRule;
use ui_core::layout::Placement;
use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};
use ui_draw::draw_list::DrawItem;
use ui_kit::tokens::MotionToken;

use super::diagnostics::GestureCounters;
use crate::action::ScreenAction;
use crate::widget::capture_blink::CaptureBlinkWidget;
use crate::widget::controls_drawer::ControlsDrawerWidget;
use crate::widget::countdown::CountdownWidget;
use crate::widget::error::WidgetError;
use crate::widget::flip_button::FlipButtonWidget;
use crate::widget::focus_indicator::FocusIndicatorWidget;
use crate::widget::frame_mask::FrameMaskWidget;
use crate::widget::geometry::ScreenGeometry;
use crate::widget::grid_overlay::GridOverlayWidget;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, VisualIndex};
use crate::widget::level_indicator::LevelIndicatorWidget;
use crate::widget::lock_banner::LockBannerWidget;
use crate::widget::menu_button::MenuButtonWidget;
use crate::widget::mode_carousel::ModeCarouselWidget;
use crate::widget::shutter_button::ShutterWidget;
use crate::widget::text_runs::TextRuns;
use crate::widget::thumbnail::ThumbnailWidget;
use crate::widget::tool_card::ToolCardWidget;
use crate::widget::top_bar::TopBarWidget;
use crate::widget::transition_veil::TransitionVeil;
use crate::widget::unavailable_notice::UnavailableNoticeWidget;
use crate::widget::zoom_strip::ZoomStripWidget;
use crate::widget::zoom_wheel::ZoomWheelWidget;
use crate::widget::{Ctx, Outputs};

pub(crate) struct Stage {
    kit: SceneKit,
    geometry: Arc<ScreenGeometry>,
    surface: SurfaceSize,
    px_per_pt_bits: u32,
    shutter: ShutterWidget,
    thumbnail: ThumbnailWidget,
    menu_button: MenuButtonWidget,
    carousel: ModeCarouselWidget,
    top_bar: TopBarWidget,
    drawer: ControlsDrawerWidget,
    card: ToolCardWidget,
    focus: FocusIndicatorWidget,
    banner: LockBannerWidget,
    flip: FlipButtonWidget,
    zoom_strip: ZoomStripWidget,
    zoom_wheel: ZoomWheelWidget,
    /// Кэш раскладки текста этой сцены: подписи раскладываются один раз (см. `widget::text_runs`).
    texts: TextRuns,
}

impl Stage {
    /// Строит сцену. `ctx.state.layout()` обязан быть (размер surface уже известен редьюсеру).
    pub(crate) fn build(
        ctx: &Ctx<'_>,
        surface: SurfaceSize,
        px_per_pt: f32,
        geometry: Arc<ScreenGeometry>,
    ) -> Result<Self, WidgetError> {
        let settings = SceneSettings {
            point_scale: ctx.config.layout.point_scale,
            motion: ctx.motion,
            ..SceneSettings::default()
        };
        let geometry_px = SurfaceGeometry {
            width_px: surface.width_px,
            height_px: surface.height_px,
            safe_area: Insets::ZERO,
        };
        let scene: UiScene<ScreenGesture> = UiScene::new(geometry_px, settings)?;
        let mut kit = SceneKit::new(scene);
        let root = kit.scene().tree().root();
        // Порядок добавления = порядок наложения: видоискатель под всем, затем элементы снизу вверх.
        let viewfinder = Self::build_viewfinder(&mut kit, root)?;
        let focus = FocusIndicatorWidget::build(&mut kit, ctx, viewfinder)?;
        let top_bar = TopBarWidget::build(&mut kit, ctx, root)?;
        // Колесо зума лежит под нижним рядом: затвор, миниатюра и капсула остаются нажимаемыми поверх него.
        let zoom_wheel = ZoomWheelWidget::build(&mut kit, ctx, root)?;
        let shutter = ShutterWidget::build(&mut kit, ctx, root)?;
        let thumbnail = ThumbnailWidget::build(&mut kit, ctx, root)?;
        let flip = FlipButtonWidget::build(&mut kit, ctx, root)?;
        let carousel = ModeCarouselWidget::build(&mut kit, ctx, root)?;
        let zoom_strip = ZoomStripWidget::build(&mut kit, ctx, root)?;
        let menu_button = MenuButtonWidget::build(&mut kit, ctx, root)?;
        let drawer = ControlsDrawerWidget::build(&mut kit, ctx, root)?;
        // Карточка — над шторкой и в хит-тесте (шторка под ней ждёт «‹» и касаний не ловит).
        let card = ToolCardWidget::build(&mut kit, ctx, root)?;
        let banner = LockBannerWidget::build(ctx)?;
        kit.relayout_if_needed()?;
        Ok(Self {
            kit,
            geometry,
            surface,
            px_per_pt_bits: px_per_pt.to_bits(),
            shutter,
            thumbnail,
            menu_button,
            carousel,
            top_bar,
            drawer,
            card,
            focus,
            banner,
            flip,
            zoom_strip,
            zoom_wheel,
            texts: TextRuns::new(ctx.config.widgets.text_run_cache_capacity),
        })
    }

    /// Видоискатель: весь экран, фон для тапа/долгого тапа и жестов режимов/меню (`behavior.md` 1.2).
    fn build_viewfinder(kit: &mut SceneKit, root: NodeId) -> Result<NodeId, WidgetError> {
        let node = kit.scene_mut().insert_child(root, "viewfinder")?;
        kit.scene_mut()
            .set_placement(node, Placement::Anchored(AnchorRule::fill_parent()))?;
        let tap = kit.add_recognizer(
            node,
            RecognizerSpec::Tap(TapSpec::default()),
            ScreenGesture::ViewfinderTap,
        )?;
        let long_press = kit.add_recognizer(
            node,
            RecognizerSpec::LongPress(LongPressSpec::default()),
            ScreenGesture::ViewfinderLongPress,
        )?;
        let mut single_finger = vec![tap, long_press];
        for (direction, intent) in [
            (PanDirection::Horizontal, ScreenGesture::ModesPan),
            (PanDirection::Up, ScreenGesture::MenuOpenPan),
        ] {
            single_finger.push(kit.add_recognizer(
                node,
                RecognizerSpec::Pan(PanSpec {
                    direction,
                    ..PanSpec::default()
                }),
                intent,
            )?);
        }
        // Щипок (зум). Два пальца обязаны отдать касание щипку, даже если один палец уже начал пан или долгий тап.
        // Однопальцевые жесты (`ExtraFingers::FailOrCancel`) второй палец сам проваливает или отменяет, но
        // победитель-пан при старте проваливает остальных кандидатов, в том числе ещё не начавшийся щипок. Поэтому
        // щипок объявлен «одновременным» с каждым однопальцевым жестом узла: он остаётся кандидатом и стартует,
        // когда ложится второй палец. Один палец щипок не запускает (нужна пара). Проверено в
        // `screen_zoom_gesture_tests`: щипок после начатого пана, щипок с нуля, пан одним пальцем зум не трогает.
        let pinch = kit.add_recognizer(
            node,
            RecognizerSpec::Pinch(PinchSpec::default()),
            ScreenGesture::ViewfinderPinch,
        )?;
        for recognizer in single_finger {
            kit.scene_mut()
                .allow_simultaneous(pinch, recognizer)
                .map_err(|error| WidgetError::Inconsistent(format!("щипок и однопальцевый жест: {error}")))?;
        }
        Ok(node)
    }

    pub(crate) fn matches(&self, surface: SurfaceSize, px_per_pt: f32) -> bool {
        self.surface == surface && self.px_per_pt_bits == px_per_pt.to_bits()
    }

    pub(crate) fn kit(&self) -> &SceneKit {
        &self.kit
    }

    pub(crate) fn kit_mut(&mut self) -> &mut SceneKit {
        &mut self.kit
    }

    /// Общая ссылка на геометрию: контекст виджетов держит её, пока сам этап изменяется.
    pub(crate) fn geometry(&self) -> Arc<ScreenGeometry> {
        Arc::clone(&self.geometry)
    }

    pub(crate) fn focus(&self) -> &FocusIndicatorWidget {
        &self.focus
    }

    #[cfg(test)]
    pub(crate) fn card(&self) -> &ToolCardWidget {
        &self.card
    }

    /// Событие жеста → виджет. `false` — события не взял никто (ошибка сборки, считается).
    pub(crate) fn route(
        &mut self,
        event: &GestureEvent<ScreenGesture>,
        ctx: &Ctx<'_>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::ViewfinderTap => {
                if let (GestureState::Ended, GestureData::Tap { position, .. }) = (event.state, event.data) {
                    out.actions.push(ScreenAction::ViewfinderTapped {
                        point: position,
                        at: event.time,
                    });
                }
                return Ok(true);
            }
            ScreenGesture::ViewfinderLongPress => {
                if let (GestureState::Began, GestureData::LongPress { position, .. }) = (event.state, event.data) {
                    out.actions.push(ScreenAction::ViewfinderLongPressed {
                        point: position,
                        at: event.time,
                    });
                }
                return Ok(true);
            }
            ScreenGesture::ViewfinderPinch => {
                Self::route_pinch(event, out);
                return Ok(true);
            }
            // Тап-«страж» кнопки нужен только арбитражу: действие рождает её `Press`.
            ScreenGesture::ButtonTapGuard(_) => return Ok(true),
            _ => {}
        }
        Ok(self.shutter.handle_gesture(event, out)?
            || self.flip.handle_gesture(event, out)?
            || self.zoom_strip.handle_gesture(event, out)?
            || self.zoom_wheel.handle_gesture(event, out)?
            || self.thumbnail.handle_gesture(event, out)?
            || self.menu_button.handle_gesture(event, out)?
            || self.carousel.handle_gesture(event, ctx, out)?
            || self.top_bar.handle_gesture(event, out)?
            || self.drawer.handle_gesture(event, out)?
            || self.card.handle_gesture(event, out)?
            || self.focus.handle_gesture(event, ctx, out)?)
    }

    /// Щипок → действия зума: начало запоминает коэффициент, каждое изменение несёт масштаб жеста.
    fn route_pinch(event: &GestureEvent<ScreenGesture>, out: &mut Outputs) {
        let at = event.time;
        let scale = match event.data {
            GestureData::Pinch { scale, .. } => Some(scale),
            _ => None,
        };
        match (event.state, scale) {
            (GestureState::Began, Some(scale)) => {
                out.actions.push(ScreenAction::ZoomPinchBegan { at });
                out.actions.push(ScreenAction::ZoomPinchChanged { scale, at });
            }
            (GestureState::Changed, Some(scale)) => out.actions.push(ScreenAction::ZoomPinchChanged { scale, at }),
            (GestureState::Ended, scale) => {
                if let Some(scale) = scale {
                    out.actions.push(ScreenAction::ZoomPinchChanged { scale, at });
                }
                out.actions.push(ScreenAction::ZoomPinchEnded { at });
            }
            // Системная отмена: значение остаётся там, куда дошло; щипок закрывается без последнего изменения.
            (GestureState::Cancelled, _) => out.actions.push(ScreenAction::ZoomPinchEnded { at }),
            _ => {}
        }
    }

    /// Выравнивает элементы и узлы по состоянию экрана (программно, без эха действий).
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.shutter.sync(&mut self.kit, ctx)?;
        self.thumbnail.sync(&mut self.kit, ctx)?;
        self.flip.sync(&mut self.kit, ctx)?;
        self.zoom_strip.sync(&mut self.kit, ctx)?;
        self.zoom_wheel.sync(&mut self.kit, ctx)?;
        self.menu_button.sync(&mut self.kit, ctx)?;
        self.carousel.sync(&mut self.kit, ctx)?;
        self.top_bar.sync(&mut self.kit, ctx)?;
        self.drawer.sync(&mut self.kit, ctx)?;
        self.card.sync(&mut self.kit, ctx)?;
        self.focus.sync(&mut self.kit, ctx)?;
        self.banner.sync(ctx)?;
        // Поворот значков по ориентации: пружина `smooth`, рамки раскладки не меняются.
        self.kit.rotate_icons(
            ctx.state.icon_rotation_clockwise_degrees(),
            ctx.spring(MotionToken::Smooth)?,
            ctx.now,
        )?;
        self.kit.relayout_if_needed()
    }

    /// Кадр элементов: пружины и события (хаптика, смена значения, выбор режима).
    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) -> Result<(), WidgetError> {
        self.kit.scene_mut().apply_animations();
        self.carousel.advance(out);
        self.drawer.advance(ctx);
        self.card.advance(ctx, out);
        self.zoom_wheel.advance(ctx, out);
        self.focus.advance(ctx, out)?;
        self.banner.advance(ctx)?;
        Ok(())
    }

    /// Список рисования кадра в порядке слоёв.
    pub(crate) fn draw(
        &mut self,
        ctx: &Ctx<'_>,
        blink: &CaptureBlinkWidget,
        veil: &TransitionVeil,
    ) -> Result<Vec<DrawItem>, WidgetError> {
        let visuals = VisualIndex::of(self.kit.scene());
        let mut layers = DrawLayers::default();
        FrameMaskWidget::draw(ctx, &mut layers)?;
        // Завеса перехода между камерами — первой над кадром: сетка и уровень лежат поверх размытия.
        veil.draw(ctx, &mut layers)?;
        GridOverlayWidget::draw(ctx, &mut layers)?;
        LevelIndicatorWidget::draw(ctx, &mut layers)?;
        blink.draw(ctx, &mut layers)?;
        self.focus.draw(ctx, &mut layers)?;
        self.banner.draw(ctx, &mut layers, &mut self.texts)?;
        // Колесо — до нижнего ряда: затвор и кнопки рисуются поверх его диска.
        self.zoom_wheel.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        self.zoom_strip.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        self.shutter.draw(ctx, &visuals, &mut layers)?;
        self.thumbnail.draw(ctx, &visuals, &mut layers)?;
        self.flip.draw(ctx, &visuals, &mut layers)?;
        self.menu_button
            .draw(ctx, &visuals, &mut layers, self.drawer.source_opacity(ctx.now))?;
        self.carousel.draw(ctx, &visuals, &mut layers)?;
        self.top_bar.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        CountdownWidget::draw(ctx, &mut layers, &mut self.texts)?;
        UnavailableNoticeWidget::draw(ctx, &mut layers, &mut self.texts)?;
        // Общий принцип: любая панель со стеклом (шторка меню, нижняя карточка) лежит над нижней зоной, и
        // миниатюра, капсула, кнопка меню не рисуются поверх её стекла ни при каких размерах и анимациях.
        // Каждая панель собирается отдельным набором слоёв и кладётся сверху; карточка — последней, над
        // шторкой, и её содержимое идёт после её стекла. Затвор рисуется в общем ряду, а не панелью:
        // над карточкой он стоит выше её рамки (поднят), поэтому стекло карточки его не накрывает.
        let mut drawer_layers = DrawLayers::default();
        self.drawer.draw(
            ctx,
            &visuals,
            &mut drawer_layers,
            &mut self.texts,
            self.card.cover_progress(ctx.now),
        )?;
        layers.stack_on_top(drawer_layers);
        let mut card_layers = DrawLayers::default();
        self.card.draw(ctx, &visuals, &mut card_layers, &mut self.texts)?;
        layers.stack_on_top(card_layers);
        Ok(layers.into_items())
    }

    /// Счётчик вида жеста для диагностики.
    pub(crate) fn count_gesture(counters: &mut GestureCounters, event: &GestureEvent<ScreenGesture>) {
        if event.state == GestureState::Cancelled {
            counters.cancelled += 1;
            return;
        }
        match (event.intent, event.state) {
            (ScreenGesture::ViewfinderTap | ScreenGesture::CapsuleTap, GestureState::Ended) => counters.taps += 1,
            (ScreenGesture::ViewfinderLongPress, GestureState::Began) => counters.long_presses += 1,
            (
                ScreenGesture::ModesPan
                | ScreenGesture::MenuOpenPan
                | ScreenGesture::SunPan
                | ScreenGesture::DrawerClosePan
                | ScreenGesture::ExposureSliderPan
                | ScreenGesture::ZoomStripPan
                | ScreenGesture::ZoomWheelPan,
                GestureState::Began,
            ) => counters.pans += 1,
            (ScreenGesture::ViewfinderPinch, GestureState::Began) => counters.pinches += 1,
            (ScreenGesture::Press(_), GestureState::Ended) => counters.button_presses += 1,
            _ => {}
        }
    }

    /// Значения шкал EV, не ставшие величиной компенсации (диагностика).
    pub(crate) fn invalid_exposure_values(&self) -> u64 {
        self.card.invalid_bias_values() + self.focus.invalid_bias_values()
    }

    pub(crate) fn text_layouts(&self) -> u64 {
        self.texts.layouts_performed()
    }

    pub(crate) fn text_cache_evictions(&self) -> u64 {
        self.texts.evictions()
    }

    /// Жесты сцены отменяются явным разрывом потока: сцену сейчас заменят (смена размера или масштаба), и
    /// палец, идущий по старой геометрии, не должен молча пропасть или «доехать» до новой. Возвращает число
    /// отменённых жестов; каждый учтён в `counters.cancelled`.
    pub(crate) fn cancel_gestures(&mut self, counters: &mut GestureCounters) -> u64 {
        self.kit.scene_mut().handle_pointer_gap();
        let mut cancelled = 0;
        for event in self.kit.scene_mut().drain_gesture_events() {
            Self::count_gesture(counters, &event);
            if event.state == GestureState::Cancelled {
                cancelled += 1;
            }
        }
        cancelled
    }

    pub(crate) fn echo_suppressed(&self) -> u64 {
        self.card.suppressed_echo_events()
            + self.focus.suppressed_echo_events()
            + self.zoom_wheel.suppressed_echo_events()
    }
}
