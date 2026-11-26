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
use super::retired_gestures::{RetiredGestureReport, RetiredGestures};
use crate::action::ScreenAction;
use crate::widget::PHOTO_BUTTON_MATERIAL;
use crate::widget::calibration::{CalibrationSection, MenuButtonDrawing, SectionKey};
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
use crate::widget::menu_glide::{GlideDirection, GlideEndpoint, GlideFrame, MenuButtonGlide};
use crate::widget::mode_carousel::ModeCarouselWidget;
use crate::widget::problem_banner::ProblemBannerWidget;
use crate::widget::shutter_button::ShutterWidget;
use crate::widget::text_runs::TextRuns;
use crate::widget::thumbnail::ThumbnailWidget;
use crate::widget::tool_card::ToolCardWidget;
use crate::widget::top_bar::TopBarWidget;
use crate::widget::transition_veil::TransitionVeil;
use crate::widget::unavailable_notice::UnavailableNoticeWidget;
use crate::widget::white_button::WhiteButtonWidget;
use crate::widget::zoom_strip::ZoomStripWidget;
use crate::widget::zoom_wheel::ZoomWheelWidget;
use crate::widget::{Ctx, Outputs};

pub(crate) struct Stage {
    kit: SceneKit,
    geometry: Arc<ScreenGeometry>,
    surface: SurfaceSize,
    px_per_pt_bits: u32,
    /// Узел-хозяин виджетов экрана PHOTO: в режиме с разделом калибровки он скрыт целиком (и рисование, и касания).
    photo_root: NodeId,
    /// Корень сцены (родитель раздела калибровки).
    scene_root: NodeId,
    /// Показан ли сейчас экран PHOTO (отличает «надо переключить» от «уже так»).
    photo_shown: bool,
    /// Переезд кнопки меню при смене PHOTO ↔ CALIBRATE (решения 282, 295): переходное состояние сцены, не `ScreenState`.
    menu_glide: MenuButtonGlide,
    /// Первая синхронизация сцены прошла: до неё смена «режима» — это не переключение, а рождение сцены.
    glide_armed: bool,
    /// Раздел калибровки; `None` вне режима с разделом.
    calibration: Option<CalibrationSection>,
    /// Жесты, порождённые снятием раздела: их заканчивает уходящий раздел (см. `retired_gestures`).
    retired_gestures: RetiredGestures,
    shutter: ShutterWidget,
    thumbnail: ThumbnailWidget,
    menu_button: MenuButtonWidget,
    /// Кнопка белого в ряду затвора (WB35).
    white_button: WhiteButtonWidget,
    carousel: ModeCarouselWidget,
    top_bar: TopBarWidget,
    drawer: ControlsDrawerWidget,
    card: ToolCardWidget,
    focus: FocusIndicatorWidget,
    banner: LockBannerWidget,
    /// Плашка проблемы пресетов: одна на весь экран, переживает пересборку раздела калибровки.
    problem_banner: ProblemBannerWidget,
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
        // Всё, что принадлежит экрану PHOTO, лежит под одним узлом: режим с разделом калибровки прячет его целиком,
        // и затвор, лента зума, верхний ряд не ловят касаний там, где стоят колонка ручек и прибор.
        let photo_root = kit.scene_mut().insert_child(root, "photo_root")?;
        kit.scene_mut()
            .set_placement(photo_root, Placement::Anchored(AnchorRule::fill_parent()))?;
        // Сам узел касаний не ловит: пустое место экрана PHOTO принадлежит видоискателю под ним.
        kit.set_pass_through(photo_root)?;
        let top_bar = TopBarWidget::build(&mut kit, ctx, photo_root)?;
        // Колесо зума лежит под нижним рядом: затвор, миниатюра и капсула остаются нажимаемыми поверх него.
        let zoom_wheel = ZoomWheelWidget::build(&mut kit, ctx, photo_root)?;
        let shutter = ShutterWidget::build(&mut kit, ctx, photo_root)?;
        let thumbnail = ThumbnailWidget::build(&mut kit, ctx, photo_root)?;
        let flip = FlipButtonWidget::build(&mut kit, ctx, photo_root)?;
        let carousel = ModeCarouselWidget::build(&mut kit, ctx, photo_root)?;
        let zoom_strip = ZoomStripWidget::build(&mut kit, ctx, photo_root)?;
        let menu_button = MenuButtonWidget::build(&mut kit, ctx, photo_root)?;
        let white_button = WhiteButtonWidget::build(&mut kit, ctx, photo_root)?;
        let drawer = ControlsDrawerWidget::build(&mut kit, ctx, photo_root)?;
        // Карточка — над шторкой и в хит-тесте (шторка под ней ждёт «‹» и касаний не ловит).
        let card = ToolCardWidget::build(&mut kit, ctx, photo_root)?;
        let banner = LockBannerWidget::build(ctx)?;
        let problem_banner = ProblemBannerWidget::build(ctx)?;
        kit.relayout_if_needed()?;
        Ok(Self {
            kit,
            geometry,
            surface,
            px_per_pt_bits: px_per_pt.to_bits(),
            photo_root,
            scene_root: root,
            photo_shown: true,
            menu_glide: MenuButtonGlide::default(),
            glide_armed: false,
            calibration: None,
            retired_gestures: RetiredGestures::default(),
            shutter,
            thumbnail,
            menu_button,
            white_button,
            carousel,
            top_bar,
            drawer,
            card,
            focus,
            banner,
            problem_banner,
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

    /// Идёт ли мерцание ожидания или тряска отказа на кнопках белого (кнопка у затвора, «Phone» на карточке).
    pub(crate) fn white_attention_animating(&self, now: shell_contract::clock::MonotonicNanos) -> bool {
        self.white_button.is_animating(now)
            || self.card.is_animating(now)
            || self
                .calibration
                .as_ref()
                .is_some_and(|section| section.is_animating(now))
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
        // Тап-«страж» кнопки нужен только арбитражу: действие рождает её `Press` (у кнопок раздела — тоже).
        if let ScreenGesture::ButtonTapGuard(_) = event.intent {
            return Ok(true);
        }
        // Жест раздела калибровки после ухода раздела — это отмена его узлов: гасим, а не считаем «ничьим».
        if event.intent.belongs_to_calibration() {
            return match &mut self.calibration {
                Some(section) => section.handle_gesture(&self.kit, ctx, event, out),
                None => Ok(true),
            };
        }
        // В режиме с разделом калибровки фокус, AE/AF Lock и зум не живут: тап, долгий тап и щипок по видоискателю пустые.
        let calibrating = ctx.state.mode_behaviour().has_calibration_surface();
        match event.intent {
            ScreenGesture::ViewfinderTap | ScreenGesture::ViewfinderLongPress if calibrating => return Ok(true),
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
            // Свайпы видоискателя (смена режима, открытие меню) заданы в осях ПОВЕРХНОСТИ, а у раздела калибровки в альбоме
            // они повёрнуты: здесь они не действуют, свайп режимов по чистой зоне принадлежит разделу (в глазных осях).
            ScreenGesture::ModesPan | ScreenGesture::MenuOpenPan if calibrating => return Ok(true),
            // Зума в разделе калибровки нет (`ModeBehaviour::camera_controls`): щипок гасим на месте, а не шлём
            // действия, которые редьюсер всё равно отвергнет и посчитает отказами.
            ScreenGesture::ViewfinderPinch if calibrating => return Ok(true),
            ScreenGesture::ViewfinderPinch => {
                Self::route_pinch(event, out);
                return Ok(true);
            }
            _ => {}
        }
        Ok(self.shutter.handle_gesture(event, out)?
            || self.white_button.handle_gesture(event, out)?
            || self.flip.handle_gesture(event, out)?
            || self.zoom_strip.handle_gesture(event, out)?
            || self.zoom_wheel.handle_gesture(event, out)?
            || self.thumbnail.handle_gesture(event, out)?
            || self.menu_button.handle_gesture(&self.kit, event, out)?
            || self.carousel.handle_gesture(&self.kit, event, out)?
            || self.top_bar.handle_gesture(event, out)?
            || self.drawer.handle_gesture(event, out)?
            || self.card.handle_gesture(&self.kit, event, out)?
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
        self.sync_calibration(ctx)?;
        self.shutter.sync(&mut self.kit, ctx)?;
        self.white_button.sync(&mut self.kit, ctx)?;
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
        self.problem_banner.sync(ctx)?;
        // Пока кнопка меню едет между режимами, ни одна из статических копий касаний не ловит.
        if self.menu_glide.is_active(ctx.now) {
            self.menu_button.suspend_touches(&mut self.kit)?;
            if let Some(section) = &self.calibration {
                section.suspend_menu_button_touches(&mut self.kit)?;
            }
        }
        self.glide_armed = true;
        // Поворот значков по ориентации: пружина `smooth`, рамки раскладки не меняются.
        self.kit.rotate_icons(
            ctx.state.icon_rotation_clockwise_degrees(),
            ctx.spring(MotionToken::Smooth)?,
            ctx.now,
        )?;
        self.kit.relayout_if_needed()
    }

    /// Приводит раздел калибровки и видимость экрана PHOTO в соответствие с режимом: раздел строится, когда у
    /// состояния появилась его раскладка, и пересобирается при смене ключа (поворот, размер, набор полос).
    fn sync_calibration(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wanted = SectionKey::of(ctx);
        // Где кнопка меню раздела стоит ДО возможной пересборки: уходящий раздел убирается ниже, а переезду в PHOTO
        // нужно место, откуда кнопка поедет.
        let section_button_before = self
            .calibration
            .as_ref()
            .and_then(|section| section.menu_button_glide_endpoint(&self.kit));
        let rebuild = match (&self.calibration, &wanted) {
            (Some(section), Some(key)) => section.key() != key,
            (None, None) => false,
            _ => true,
        };
        if rebuild {
            if let Some(mut section) = self.calibration.take() {
                section.retire(&mut self.kit)?;
                // `Cancelled` идущих жестов уходящего раздела заканчивает он сам, а не новый раздел.
                self.retired_gestures.settle(&mut section, &mut self.kit, ctx)?;
            }
            if let Some(key) = wanted {
                self.calibration = Some(CalibrationSection::build(&mut self.kit, ctx, self.scene_root, key)?);
            }
            self.kit.relayout_if_needed()?;
        }
        let photo_wanted = !ctx.state.mode_behaviour().has_calibration_surface();
        if self.photo_shown != photo_wanted {
            if self.glide_armed {
                self.start_menu_glide(ctx, photo_wanted, section_button_before)?;
            }
            self.photo_shown = photo_wanted;
            self.kit.set_shown(self.photo_root, photo_wanted)?;
            if photo_wanted {
                // `set_shown` возвращает обычную политику касаний; узлу-хозяину нужна «прозрачная».
                self.kit.set_pass_through(self.photo_root)?;
            }
        }
        if let Some(section) = &mut self.calibration {
            section.sync(&mut self.kit, ctx)?;
        }
        Ok(())
    }

    /// Смена PHOTO ↔ CALIBRATE: кнопка меню едет от своего места в прежнем режиме к месту в новом. `photo_wanted` —
    /// куда переключаемся; `section_button_before` — где кнопка раздела стояла до пересборки (раздел уже мог уйти).
    /// Нет места-источника (узла нет в дереве) — переезда нет, кнопка просто окажется на новом месте.
    fn start_menu_glide(
        &mut self,
        ctx: &Ctx<'_>,
        photo_wanted: bool,
        section_button_before: Option<GlideEndpoint>,
    ) -> Result<(), WidgetError> {
        let (direction, from) = if photo_wanted {
            (GlideDirection::SectionToPhoto, section_button_before)
        } else {
            (
                GlideDirection::PhotoToSection,
                self.menu_button.glide_endpoint(&self.kit),
            )
        };
        match from {
            Some(from) => self.menu_glide.start(ctx, direction, from),
            None => Ok(()),
        }
    }

    /// Где рисовать ездящую кнопку меню в этот кадр; `None` — переезда нет. Конец-назначение берётся у дома заново
    /// каждый кадр: корень раздела может ещё поворачиваться, и кнопка садится ровно в якорь раскладки.
    fn menu_glide_frame(&mut self, ctx: &Ctx<'_>) -> Option<GlideFrame> {
        let direction = self.menu_glide.direction(ctx.now)?;
        let target = match direction {
            GlideDirection::PhotoToSection => self.calibration.as_ref()?.menu_button_glide_endpoint(&self.kit)?,
            GlideDirection::SectionToPhoto => self.menu_button.glide_endpoint(&self.kit)?,
        };
        self.menu_glide.frame(ctx.now, target)
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
        self.problem_banner.advance(ctx, out)?;
        if let Some(section) = &mut self.calibration {
            section.advance(ctx, out);
        }
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
        let glide = self.menu_glide_frame(ctx);
        if ctx.state.mode_behaviour().has_calibration_surface() {
            return self.draw_calibration(ctx, &visuals, layers, (blink, veil), glide);
        }
        FrameMaskWidget::draw(ctx, &mut layers)?;
        // Завеса перехода между камерами — первой над кадром: сетка и уровень лежат поверх размытия.
        veil.draw(ctx, &mut layers)?;
        GridOverlayWidget::draw(ctx, &mut layers)?;
        LevelIndicatorWidget::draw(ctx, &mut layers)?;
        blink.draw(ctx, &mut layers)?;
        self.focus.draw(ctx, &mut layers)?;
        self.banner.draw(ctx, &mut layers, &mut self.texts)?;
        self.problem_banner.draw(ctx, &mut layers, &mut self.texts, |size| {
            Some(ProblemBannerWidget::photo_placement(ctx, size))
        })?;
        // Колесо — до нижнего ряда: затвор и кнопки рисуются поверх его диска.
        self.zoom_wheel.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        self.zoom_strip.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        self.shutter.draw(ctx, &visuals, &mut layers)?;
        self.thumbnail.draw(ctx, &visuals, &mut layers)?;
        self.flip.draw(ctx, &visuals, &mut layers)?;
        match &glide {
            // Идёт переезд: кнопку рисует ездящая копия, статическая не рисуется (двойной кнопки нет).
            Some(frame) => MenuButtonWidget::draw_gliding(ctx, &mut layers, frame)?,
            None => self.menu_button.draw(
                ctx,
                &visuals,
                &mut layers,
                self.drawer.source_opacity(ctx.now),
                PHOTO_BUTTON_MATERIAL,
            )?,
        }
        self.white_button.draw(ctx, &visuals, &mut layers, &mut self.texts)?;
        self.carousel.draw(ctx, &visuals, &mut layers, PHOTO_BUTTON_MATERIAL)?;
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

    /// Список рисования режима с разделом калибровки: видоискатель на весь экран (его рисует рендер), поверх —
    /// раздел. Рамка кадра, сетка, уровень, рамка фокуса, плашка Lock и вся нижняя зона PHOTO не рисуются.
    fn draw_calibration(
        &mut self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        mut layers: DrawLayers,
        (blink, veil): (&CaptureBlinkWidget, &TransitionVeil),
        glide: Option<GlideFrame>,
    ) -> Result<Vec<DrawItem>, WidgetError> {
        veil.draw(ctx, &mut layers)?;
        blink.draw(ctx, &mut layers)?;
        UnavailableNoticeWidget::draw(ctx, &mut layers, &mut self.texts)?;
        if let Some(section) = &self.calibration {
            let menu_button = if glide.is_some() {
                MenuButtonDrawing::Suppressed
            } else {
                MenuButtonDrawing::Static
            };
            section.draw(ctx, visuals, &mut layers, &mut self.texts, menu_button)?;
        }
        if let Some(frame) = &glide {
            MenuButtonWidget::draw_gliding(ctx, &mut layers, frame)?;
        }
        // Плашка проблемы — в глазном пространстве раздела (у верха нижнего ряда), поверх раздела и его меню.
        let section = self.calibration.as_ref();
        self.problem_banner.draw(ctx, &mut layers, &mut self.texts, |size| {
            section.and_then(|section| section.banner_placement(ctx, visuals, size))
        })?;
        Ok(layers.into_items())
    }

    /// Счётчик вида жеста для диагностики.
    pub(crate) fn count_gesture(counters: &mut GestureCounters, event: &GestureEvent<ScreenGesture>) {
        if event.state == GestureState::Cancelled {
            counters.cancelled += 1;
            return;
        }
        match (event.intent, event.state) {
            (
                ScreenGesture::ViewfinderTap
                | ScreenGesture::CapsuleTap
                | ScreenGesture::CalibrationBaseTap
                | ScreenGesture::CalibrationMenuScrimTap,
                GestureState::Ended,
            ) => counters.taps += 1,
            (ScreenGesture::ViewfinderLongPress | ScreenGesture::WhiteButtonLongPress, GestureState::Began) => {
                counters.long_presses += 1
            }
            (
                ScreenGesture::ModesPan
                | ScreenGesture::MenuOpenPan
                | ScreenGesture::SunPan
                | ScreenGesture::DrawerClosePan
                | ScreenGesture::ExposureSliderPan
                | ScreenGesture::WhiteRulerPan
                | ScreenGesture::ZoomStripPan
                | ScreenGesture::ZoomWheelPan
                | ScreenGesture::CalibrationRulerPan(_)
                | ScreenGesture::CalibrationWheelPadPan
                | ScreenGesture::CalibrationListPan
                | ScreenGesture::CalibrationBasePan
                | ScreenGesture::CalibrationScopePan
                | ScreenGesture::CalibrationModesPan
                | ScreenGesture::StyleChipsPan
                | ScreenGesture::CalibrationChipsPan
                | ScreenGesture::CalibrationMenuClosePan
                | ScreenGesture::SectionWhiteRulerPan,
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

    /// Значения шкал белого, не принятые как невозможные (диагностика).
    pub(crate) fn invalid_white_values(&self) -> u64 {
        self.card.invalid_white_values()
    }

    /// Центры (pt, поверхность) всех видимых узлов с подписью `label`, в порядке сборки (проба для тестов касаний и
    /// цели касаний раздела калибровки, см. `calibration_targets`).
    pub(crate) fn node_centers(&self, label: &str) -> Vec<ui_core::geometry::Point> {
        self.kit
            .scene()
            .tree()
            .draw_order()
            .into_iter()
            .filter(|visual| visual.label == label)
            .map(|visual| visual.to_screen.apply(visual.frame.center()))
            .collect()
    }

    /// Сцена целиком (проба для тестов соседства областей касания: хит-тест и рамки узлов).
    #[cfg(test)]
    pub(crate) fn scene(&self) -> &ui_core::scene::UiScene<crate::widget::intent::ScreenGesture> {
        self.kit.scene()
    }

    /// Раздел калибровки (проба для тестов).
    #[cfg(test)]
    pub(crate) fn calibration(&self) -> Option<&CalibrationSection> {
        self.calibration.as_ref()
    }

    /// Значения ручек вида, не ставшие значениями (диагностика).
    /// События жестов к разбору: сначала отложенные при снятии раздела, затем накопленные сценой.
    pub(crate) fn drain_gesture_events(&mut self) -> Vec<GestureEvent<ScreenGesture>> {
        let mut events = self.retired_gestures.take_deferred();
        events.extend(self.kit.scene_mut().drain_gesture_events());
        events
    }

    /// Итог жестов, законченных уходящими разделами с прошлого забора (выход виджетов и счётчики).
    pub(crate) fn take_retired_gestures(&mut self) -> RetiredGestureReport {
        self.retired_gestures.take_report()
    }

    /// Продолжения пана без начала: у живого раздела и у уже убранных (диагностика, на нормальном пути 0).
    pub(crate) fn orphan_pan_continuations(&self) -> u64 {
        self.retired_gestures.orphan_pan_continuations_of_retired()
            + self
                .calibration
                .as_ref()
                .map_or(0, CalibrationSection::orphan_pan_continuations)
    }

    /// Отказы «картинки под пальцем» (пустой список ведомых узлов): у живого раздела и у уже убранных.
    pub(crate) fn finger_focus_refusals(&self) -> u64 {
        self.retired_gestures.finger_focus_refusals_of_retired()
            + self
                .calibration
                .as_ref()
                .map_or(0, CalibrationSection::finger_focus_refusals)
    }

    pub(crate) fn invalid_knob_values(&self) -> u64 {
        self.calibration.as_ref().map_or(0, CalibrationSection::invalid_values)
    }

    /// Имена пресетов, не поместившиеся в ряды таблеток (диагностика): в карточке STYLES и в меню раздела.
    pub(crate) fn overflowed_preset_chips(&self) -> u64 {
        let menu = self
            .calibration
            .as_ref()
            .map_or(0, CalibrationSection::overflowed_chips);
        (self.card.overflowed_style_chips() + menu) as u64
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
        for event in self.drain_gesture_events() {
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
            + self
                .calibration
                .as_ref()
                .map_or(0, CalibrationSection::suppressed_echo_events)
    }
}
