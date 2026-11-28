//! Виджеты раздела калибровки (режим CALIBRATE, решения 185, 196–203, 255–261, 274–283).
//!
//! Раздел — ОДНА сборка под одним повёрнутым корнем (`SectionSpace`): колонка ручек на тёмной поверхности (сторону
//! задаёт конфиг), рядом чистая зона (ничего не рисуется и не ловит касаний — под ней живой видоискатель), прибор
//! (полоска с именем, точками и осями + картинка с числами уровней) и нижний ряд: CLEAN слева, капсула режимов и
//! кнопка меню справа. Раскладка и поворот по ориентации телефона приходят от `CalibrationLayout`, раздел сам ничего
//! не считает.
//!
//! # Жизненный цикл
//! Раздел существует, пока у состояния есть раскладка раздела (режим CALIBRATE, известен размер surface), и
//! пересобирается целиком при смене ключа [`SectionKey`]: раскладки (поворот телефона, размер) или набора полос
//! (их точки строятся по фактам). Всё, что раздел показывает, берётся из фактов и выбора экрана, поэтому
//! пересборка ничего не теряет, кроме положения прокрутки списка. Распознаватели удалённых узлов получают
//! `Cancelled`, а «чистяк», оставшийся без пальца, сам отпускается (`CleanButton::advance`).
//!
//! # Что придумано, а не измерено
//! В эталоне Apple нет раздела калибровки (решение 96): колонка на поверхности, строки ручек, CLEAN, точки полос и страниц прибора,
//! листание приборов — идиомы владельца и подгонка. Измерено только то, что взято у карточки Exposure: вид
//! линейки и кнопки «↻» (яркая / 44 %). Числа вида — `CalibrationWidgetsConfig`.

mod band_dots;
mod bars;
mod clean_button;
mod clean_zone;
mod control_view;
mod finger_focus;
mod host;
mod knob_row;
mod knob_scale;
mod knob_stops;
mod knobs_column;
mod menu;
mod paint;
mod primary_row;
mod reset_disc;
mod scope_labels_draw;
mod scope_panel;
mod scope_strip;
mod segments;
mod selection_haptic;
pub(crate) mod space;
mod tone_header;
mod wheels;
mod white_capsule;

use look_contract::HueBandId;
use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_kit::carousel::CarouselMotion;

use self::clean_button::CleanButton;
use self::knobs_column::KnobsColumn;
use self::menu::SectionCards;
use self::scope_panel::ScopePanel;
use self::space::SectionSpace;
use super::SECTION_PANEL_MATERIAL;
use super::error::WidgetError;
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, VisualIndex};
use super::menu_button::{MenuButtonHome, MenuButtonWidget};
use super::menu_glide::GlideEndpoint;
use super::mode_carousel::{CarouselHome, ModeCarouselWidget};
use super::text_runs::TextRuns;
use super::{Ctx, Outputs};
use crate::calibration::{CalibrationLayout, ControlViewKind};

/// От чего зависит устройство раздела: пока ключ тот же, узлы живут; изменился — раздел пересобирается.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SectionKey {
    layout: CalibrationLayout,
    bands: Vec<HueBandId>,
    /// Вид колонки ручек: шкалы и колёса — разные наборы узлов, смена вида пересобирает раздел (решение 204).
    /// Выбранное колесо в ключ НЕ входит: вкладки переключаются на месте.
    control_view: ControlViewKind,
}

impl SectionKey {
    /// Ключ по состоянию экрана; `None` — раздела сейчас нет (не тот режим или нет размера surface).
    pub(crate) fn of(ctx: &Ctx<'_>) -> Option<Self> {
        let layout = *ctx.state.calibration().layout()?;
        let bands = ctx
            .state
            .look_facts()
            .map(|facts| facts.bands.iter().map(|band| band.id.clone()).collect())
            .unwrap_or_default();
        Some(Self {
            layout,
            bands,
            control_view: ctx.state.calibration().control_view(),
        })
    }
}

/// Рисовать ли статическую кнопку меню раздела.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuButtonDrawing {
    /// Обычно: кнопка стоит в своём якоре.
    Static,
    /// Идёт переезд PHOTO ↔ CALIBRATE: кнопку рисует ездящая копия.
    Suppressed,
}

/// Сборка раздела калибровки.
pub(crate) struct CalibrationSection {
    key: SectionKey,
    space: SectionSpace,
    root: NodeId,
    knobs: KnobsColumn,
    scope: ScopePanel,
    /// CLEAN внизу слева, зеркально кнопке меню (решение 259).
    clean: CleanButton,
    carousel: ModeCarouselWidget,
    menu_button: MenuButtonWidget,
    /// Завеса и нижние карточки (пресеты, белое): лежат над всем остальным разделом.
    cards: SectionCards,
}

impl CalibrationSection {
    /// Строит раздел по ключу внутри корня сцены `scene_root`.
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        scene_root: NodeId,
        key: SectionKey,
    ) -> Result<Self, WidgetError> {
        let screen = ctx.geometry.screen();
        let space = SectionSpace::new(&key.layout, screen);
        let root = kit.add_node(scene_root, "calibration_root", space.root_frame(), screen)?;
        // Корень шире экрана (альбом) и сам не ловит касаний: чистая зона отдаёт их видоискателю под раскладкой.
        kit.set_pass_through(root)?;
        kit.jump_rotation_to(root, space.rotation_radians())?;
        let root_frame = space.root_frame();
        let zones = *key.layout.zones();
        let bands = ctx.state.look_facts().map_or(&[][..], |facts| facts.bands.as_slice());
        let knobs = KnobsColumn::build(
            kit,
            ctx,
            &space,
            (root, root_frame),
            (space.scene_rect(zones.knobs), bands, key.control_view),
        )?;
        let scope = ScopePanel::build(kit, ctx, &space, (root, root_frame), &key.layout)?;
        // Чистая зона: невидимый узел свайпа режимов в глазных осях (в альбоме жесты видоискателя идут в осях поверхности).
        clean_zone::build_mode_swipe_zone(kit, &space, (root, root_frame), space.scene_rect(zones.clean_preview))?;
        // Капсула раздела — тот же измеренный размер, что на экране PHOTO (221,4 × 48,9 pt): соседняя подпись
        // видна у края и гаснет в окне капсулы.
        let capsule_frame = Rect::centered_at(
            space.scene_point(key.layout.carousel_center()),
            ctx.geometry.capsule().size,
        );
        let carousel = ModeCarouselWidget::build_at(
            kit,
            ctx,
            (root, root_frame),
            CarouselHome::Section {
                frame: capsule_frame,
                space,
            },
        )?;
        let clean = CleanButton::build(
            kit,
            ctx,
            (root, root_frame),
            space.scene_point(key.layout.clean_button_center()),
        )?;
        let menu_frame = Rect::centered_at(
            space.scene_point(key.layout.menu_button_center()),
            ctx.geometry.menu_button().size,
        );
        let menu_button = MenuButtonWidget::build_at(
            kit,
            ctx,
            (root, root_frame),
            MenuButtonHome::Section { frame: menu_frame },
        )?;
        // Меню строится последним: узлы идут в порядке наложения, и скрим с карточкой лежат над ручками.
        let cards = SectionCards::build(kit, ctx, &space, (root, root_frame))?;
        Ok(Self {
            key,
            space,
            root,
            knobs,
            scope,
            clean,
            carousel,
            menu_button,
            cards,
        })
    }

    /// Ключ, по которому построен раздел.
    pub(crate) fn key(&self) -> &SectionKey {
        &self.key
    }

    /// Убирает узлы раздела из сцены (идущие жесты получают `Cancelled`).
    pub(crate) fn retire(&self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        kit.remove_subtree(self.root)
    }

    /// Событие жеста → виджет раздела. `false` — событие не раздела.
    pub(crate) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        Ok(self.knobs.handle_gesture(kit, &self.space, ctx, event, out)?
            || self.scope.handle_gesture(kit, ctx, event, out)?
            || self.clean.handle_gesture(kit, event, out)?
            || self.carousel.handle_gesture(kit, event, out)?
            || self.menu_button.handle_gesture(kit, event, out)?
            || self.cards.handle_gesture(kit, ctx, event, out)?)
    }

    /// Выравнивает виджеты раздела по состоянию (без эха действий).
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.knobs.sync(kit, ctx)?;
        self.scope.sync(kit, ctx)?;
        self.carousel.sync(kit, ctx)?;
        self.menu_button.sync(kit, ctx)?;
        self.cards.sync(kit, ctx)
    }

    /// Кадр: события линеек, каруселей и «чистяка».
    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.knobs.advance(ctx, out);
        self.clean.advance(ctx, out);
        self.carousel.advance(out);
        self.cards.advance(ctx, out);
    }

    /// Рисует раздел. `menu_button` — рисовать ли статическую кнопку меню: на время переезда PHOTO ↔ CALIBRATE её
    /// рисует ездящая копия `Stage`, двойной кнопки быть не должно.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        menu_button: MenuButtonDrawing,
    ) -> Result<(), WidgetError> {
        self.knobs.draw(ctx, visuals, layers, texts)?;
        self.scope.draw(ctx, visuals, layers, texts)?;
        // Нижний ряд лежит ПОД открытой карточкой и сквозь её стекло не просвечивает (решение 292).
        if !self.cards.covers_bottom_row(ctx) {
            self.clean.draw(ctx, visuals, layers, texts)?;
            self.carousel.draw(ctx, visuals, layers, SECTION_PANEL_MATERIAL)?;
            if menu_button == MenuButtonDrawing::Static {
                self.menu_button
                    .draw(ctx, visuals, layers, 1.0, SECTION_PANEL_MATERIAL)?;
            }
        }
        self.cards.draw(ctx, visuals, layers, texts)
    }

    /// Что капсула режимов раздела рисует сейчас (для передачи эстафеты капсуле PHOTO); `None` — капсула в покое.
    pub(crate) fn capsule_motion(&self, ctx: &Ctx<'_>) -> Option<CarouselMotion> {
        self.carousel.motion_snapshot(ctx)
    }

    /// Капсула раздела продолжает движение капсулы PHOTO, а не стоит на своём покое.
    pub(crate) fn adopt_capsule_motion(&mut self, motion: CarouselMotion, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.carousel.adopt_motion(motion, ctx)
    }

    /// Где кнопка меню раздела стоит на экране (начало и конец переезда PHOTO ↔ CALIBRATE).
    pub(crate) fn menu_button_glide_endpoint(&self, kit: &SceneKit) -> Option<GlideEndpoint> {
        self.menu_button.glide_endpoint(kit)
    }

    /// На время переезда кнопка меню раздела касаний не ловит (после `sync`, который вернул бы обычное состояние).
    pub(crate) fn suspend_menu_button_touches(&self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        self.menu_button.suspend_touches(kit)
    }

    /// Размещение плашки проблемы размера `size_pt` в глазном пространстве раздела: по центру ширины, прижата к
    /// верху нижнего ряда или открытого меню (над ней — зоны ручек и прибора; чистая зона превью остаётся пустой,
    /// решение 203).
    /// `None`, пока у корня нет размещения.
    pub(crate) fn banner_placement(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        size_pt: ui_core::geometry::Size,
    ) -> Option<ui_draw::scene_bridge::NodePlacementPx> {
        let root_frame = self.space.root_frame();
        let host = host::RootHost::of(ctx, visuals, self.root, root_frame.center())?;
        let bottom_row = self.space.scene_rect(self.key.layout.zones().bottom_row);
        // Открытое меню лежит над нижним рядом: плашка встаёт над его верхом, а не поверх карточки.
        let anchor_y = self
            .cards
            .open_top(ctx.state.calibration().card())
            .map_or(bottom_row.min_y(), |menu_top| menu_top.min(bottom_row.min_y()));
        let gap = ctx.config.calibration.widgets.problem_banner_gap_pt;
        let center = ui_core::geometry::Point::new(root_frame.center().x, anchor_y - gap - size_pt.height / 2.0);
        Some(ui_draw::scene_bridge::NodePlacementPx {
            size: ui_draw::geometry::PxSize::new(ctx.px.px(size_pt.width), ctx.px.px(size_pt.height)),
            transform: host.point(ctx, center, 0.0),
            opacity: 1.0,
        })
    }

    /// Имена пресетов, не поместившиеся в ряд меню (диагностика).
    pub(crate) fn overflowed_chips(&self) -> usize {
        self.cards.overflowed_chips()
    }

    /// Нужны ли кадры из-за карточки белого (мерцание «Phone», тряска).
    pub(crate) fn is_animating(&self, now: ui_core::time::MonotonicNanos) -> bool {
        self.cards.is_animating(now)
    }

    /// Смещение прокрутки списка ручек, pt (проба для тестов).
    #[cfg(test)]
    pub(crate) fn scroll_offset(&self, now: ui_core::time::MonotonicNanos) -> f64 {
        self.knobs.scroll_offset(now)
    }

    /// Положение линейки строки `slot` в шагах (проба для тестов).
    #[cfg(test)]
    pub(crate) fn row_position_steps(&self, slot: u8, now: ui_core::time::MonotonicNanos) -> Option<f64> {
        self.knobs.row_position_steps(slot, now)
    }

    /// Сколько значений линеек не стали значениями вида (диагностика).
    pub(crate) fn invalid_values(&self) -> u64 {
        self.knobs.invalid_values() + self.cards.invalid_white_values()
    }

    /// Отказы «картинки под пальцем» из-за пустого списка ведомых узлов (диагностика, на нормальном пути 0).
    pub(crate) fn finger_focus_refusals(&self) -> u64 {
        self.knobs.finger_focus_refusals()
    }

    /// Продолжения пана без начала в органах раздела (диагностика, на нормальном пути 0).
    pub(crate) fn orphan_pan_continuations(&self) -> u64 {
        self.knobs.orphan_pan_continuations()
    }

    /// Сколько событий значения погашено как эхо (диагностика).
    pub(crate) fn suppressed_echo_events(&self) -> u64 {
        self.knobs.suppressed_echo_events()
    }
}
