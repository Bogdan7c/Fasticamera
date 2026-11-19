//! Сцена экрана под конкретный размер surface и масштаб: дерево, жесты и все виджеты.
//!
//! Сцена пересобирается при смене размера или масштаба (как в стенде стекла): положения пальцев и рамки
//! зависят от масштаба, поэтому старые жесты отбрасываются. Состояние экрана (`ScreenState`) при этом не
//! теряется, а элементы выравниваются по нему первой же синхронизацией.

use std::sync::Arc;

use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Insets;
use ui_core::gesture::{
    GestureData, GestureEvent, GestureState, LongPressSpec, PanDirection, PanSpec, RecognizerSpec, TapSpec,
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
use crate::widget::exposure_slider::ExposurePanelWidget;
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
use crate::widget::top_bar::TopBarWidget;
use crate::widget::unavailable_notice::UnavailableNoticeWidget;
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
    exposure: ExposurePanelWidget,
    focus: FocusIndicatorWidget,
    banner: LockBannerWidget,
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
        let shutter = ShutterWidget::build(&mut kit, ctx, root)?;
        let thumbnail = ThumbnailWidget::build(&mut kit, ctx, root)?;
        let carousel = ModeCarouselWidget::build(&mut kit, ctx, root)?;
        let menu_button = MenuButtonWidget::build(&mut kit, ctx, root)?;
        let exposure = ExposurePanelWidget::build(&mut kit, ctx, root)?;
        let drawer = ControlsDrawerWidget::build(&mut kit, ctx, root)?;
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
            exposure,
            focus,
            banner,
            texts: TextRuns::new(ctx.config.widgets.text_run_cache_capacity),
        })
    }

    /// Видоискатель: весь экран, фон для тапа/долгого тапа и жестов режимов/меню (`behavior.md` 1.2).
    fn build_viewfinder(kit: &mut SceneKit, root: NodeId) -> Result<NodeId, WidgetError> {
        let node = kit.scene_mut().insert_child(root, "viewfinder")?;
        kit.scene_mut()
            .set_placement(node, Placement::Anchored(AnchorRule::fill_parent()))?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Tap(TapSpec::default()),
            ScreenGesture::ViewfinderTap,
        )?;
        kit.add_recognizer(
            node,
            RecognizerSpec::LongPress(LongPressSpec::default()),
            ScreenGesture::ViewfinderLongPress,
        )?;
        for (direction, intent) in [
            (PanDirection::Horizontal, ScreenGesture::ModesPan),
            (PanDirection::Up, ScreenGesture::MenuOpenPan),
        ] {
            kit.add_recognizer(
                node,
                RecognizerSpec::Pan(PanSpec {
                    direction,
                    ..PanSpec::default()
                }),
                intent,
            )?;
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
    pub(crate) fn exposure(&self) -> &ExposurePanelWidget {
        &self.exposure
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
            // Тап-«страж» кнопки нужен только арбитражу: действие рождает её `Press`.
            ScreenGesture::ButtonTapGuard(_) => return Ok(true),
            _ => {}
        }
        Ok(self.shutter.handle_gesture(event, out)?
            || self.thumbnail.handle_gesture(event, out)?
            || self.menu_button.handle_gesture(event, out)?
            || self.carousel.handle_gesture(event, ctx, out)?
            || self.top_bar.handle_gesture(event, out)?
            || self.drawer.handle_gesture(event, out)?
            || self.exposure.handle_gesture(event, out)?
            || self.focus.handle_gesture(event)?)
    }

    /// Выравнивает элементы и узлы по состоянию экрана (программно, без эха действий).
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.shutter.sync(&mut self.kit, ctx)?;
        self.menu_button.sync(&mut self.kit, ctx)?;
        self.carousel.sync(&mut self.kit, ctx)?;
        self.top_bar.sync(&mut self.kit, ctx)?;
        self.drawer.sync(&mut self.kit, ctx)?;
        self.exposure.sync(&mut self.kit, ctx)?;
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
        self.exposure.advance(ctx, out)?;
        self.focus.advance(ctx, out)?;
        self.banner.advance(ctx)?;
        Ok(())
    }

    /// Список рисования кадра в порядке слоёв.
    pub(crate) fn draw(&mut self, ctx: &Ctx<'_>, blink: &CaptureBlinkWidget) -> Result<Vec<DrawItem>, WidgetError> {
        let visuals = VisualIndex::of(self.kit.scene());
        let mut layers = DrawLayers::default();
        FrameMaskWidget::draw(ctx, &mut layers)?;
        GridOverlayWidget::draw(ctx, &mut layers)?;
        LevelIndicatorWidget::draw(ctx, &mut layers)?;
        blink.draw(ctx, &mut layers)?;
        self.focus.draw(ctx, &mut layers)?;
        self.banner.draw(ctx, &mut layers, &mut self.texts)?;
        self.shutter.draw(ctx, &visuals, &mut layers)?;
        self.thumbnail.draw(ctx, &visuals, &mut layers)?;
        self.menu_button
            .draw(ctx, &visuals, &mut layers, self.drawer.source_opacity(ctx.now))?;
        self.carousel.draw(ctx, &visuals, &mut layers)?;
        self.top_bar.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        CountdownWidget::draw(ctx, &mut layers, &mut self.texts)?;
        UnavailableNoticeWidget::draw(ctx, &mut layers, &mut self.texts)?;
        // Общий принцип: любая панель со стеклом (шторка меню, панель Exposure) лежит над всей нижней зоной, и
        // затвор, кнопка меню, миниатюра не рисуются поверх её стекла ни при каких размерах и анимациях. Каждая
        // панель собирается отдельным набором слоёв и кладётся сверху (одновременно открыта одна панель).
        let mut drawer_layers = DrawLayers::default();
        self.drawer.draw(ctx, &visuals, &mut drawer_layers, &mut self.texts)?;
        layers.stack_on_top(drawer_layers);
        let mut exposure_layers = DrawLayers::default();
        self.exposure
            .draw(ctx, &visuals, &mut exposure_layers, &mut self.texts)?;
        layers.stack_on_top(exposure_layers);
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
                | ScreenGesture::ExposureSliderPan,
                GestureState::Began,
            ) => counters.pans += 1,
            (ScreenGesture::Press(_), GestureState::Ended) => counters.button_presses += 1,
            _ => {}
        }
    }

    /// Значения шкал EV, не ставшие величиной компенсации (диагностика).
    pub(crate) fn invalid_exposure_values(&self) -> u64 {
        self.exposure.invalid_bias_values() + self.focus.invalid_bias_values()
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
        self.exposure.suppressed_echo_events() + self.focus.suppressed_echo_events()
    }
}
