//! Меню «шесть точек» раздела калибровки (решения 94, 199, 201): шторка корня и две карточки.
//!
//! Всё лежит под повёрнутым корнем раздела (глазное пространство, `SectionSpace`) и строится теми же средствами,
//! что меню Controls экрана PHOTO: шторка и карточка — `PanelMorph`, пункты и «ручка» — `sheet_view`, боковые
//! кнопки — `SideButton`, ряды таблеток — `ChipRow`, числа — `widgets.sheet` и `widgets.card`. Геометрия шторки и
//! карточки берётся у `ScreenGeometry::in_screen` на глазном прямоугольнике: у его низа и середины, а не экрана.
//!
//! # Владение и инварианты
//! * Какое меню открыто, владеет `CalibrationState::menu`; виджеты выравниваются по нему в `sync` (без эха).
//! * Пока меню открыто, под ним лежат ручки и прибор: «скрим» на весь корень (первый узел меню, интерактивен только
//!   при открытом меню) ловит тап мимо и закрывает меню (аналог решения 94). Шторка и карточка лежат над ним.
//! * Закрывает меню также свайп вниз (в глазных осях) по шторке и карточке; закрытие по смещению, а не по скорости,
//!   чтобы медленный свайп тоже закрывал.
//! * Рисуется меню поверх всего раздела (`DrawLayers::stack_on_top`): шторка, затем карточка со своим стеклом.

mod card;
mod sheet;

use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureEvent, GestureState, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_kit::pan::PanUpdate;

use self::card::MenuCard;
use self::sheet::MenuSheet;
use super::host::RootHost;
use super::space::SectionSpace;
use crate::action::ScreenAction;
use crate::calibration::CalibrationMenu;
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

pub(super) struct SectionMenu {
    space: SectionSpace,
    root: NodeId,
    root_center: Point,
    scrim: NodeId,
    sheet: MenuSheet,
    card: MenuCard,
    /// Последнее смещение пальца по глазной вертикали за идущий свайп, pt (вниз — положительное).
    close_drag_dy: f64,
}

impl SectionMenu {
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        space: &SectionSpace,
        (root, root_frame): (NodeId, Rect),
    ) -> Result<Self, WidgetError> {
        // «Экран» меню — глазной прямоугольник раздела в системе корня.
        let eye = ctx.geometry.in_screen(root_frame);
        let scrim = kit.add_node(root, "calib_menu_scrim", root_frame, root_frame)?;
        kit.add_recognizer(
            scrim,
            RecognizerSpec::Tap(TapSpec::default()),
            ScreenGesture::CalibrationMenuScrimTap,
        )?;
        kit.set_interactive(scrim, false)?;
        let sheet = MenuSheet::build(kit, ctx, (&eye, space), (root, root_frame))?;
        let card = MenuCard::build(kit, ctx, (&eye, space), (root, root_frame), sheet.rect())?;
        Ok(Self {
            space: *space,
            root,
            root_center: root_frame.center(),
            scrim,
            sheet,
            card,
            close_drag_dy: 0.0,
        })
    }

    /// Верх открытой части меню в системе корня, pt: у корня меню — шторка, у карточек — карточка; `None` — меню
    /// закрыто. Плашка проблемы встаёт над этой линией, а не поверх меню.
    pub(super) fn open_top(&self, menu: CalibrationMenu) -> Option<f64> {
        match menu {
            CalibrationMenu::Closed => None,
            CalibrationMenu::Root => Some(self.sheet.rect().min_y()),
            CalibrationMenu::Presets | CalibrationMenu::ScopeAxes => Some(self.card.rect().min_y()),
        }
    }

    /// Имена пресетов, не поместившиеся в ряд карточки (диагностика).
    pub(super) fn overflowed_chips(&self) -> usize {
        self.card.overflowed_chips()
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        match event.intent {
            ScreenGesture::CalibrationMenuScrimTap => {
                // Закрывает только завершённый тап; отменённый (разрыв потока) ничего не делает.
                if let (GestureState::Ended, GestureData::Tap { .. }) = (event.state, event.data) {
                    out.actions.push(ScreenAction::ControlsMenuCloseRequested);
                }
                Ok(true)
            }
            ScreenGesture::CalibrationMenuClosePan => {
                self.handle_close_pan(event, out)?;
                Ok(true)
            }
            _ => Ok(self.sheet.handle_gesture(kit, event, out)? || self.card.handle_gesture(kit, event, out)?),
        }
    }

    /// Свайп вниз глазами закрывает меню по завершению; вверх и отменённый — ничего.
    fn handle_close_pan(&mut self, event: &GestureEvent<ScreenGesture>, out: &mut Outputs) -> Result<(), WidgetError> {
        let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан меню раздела"))?
        else {
            return Ok(());
        };
        match self.space.eye_pan(update) {
            PanUpdate::Began { translation } | PanUpdate::Changed { translation } => {
                self.close_drag_dy = translation.from_recognition.dy;
            }
            PanUpdate::Ended { .. } => {
                if self.close_drag_dy > 0.0 {
                    out.actions.push(ScreenAction::ControlsMenuCloseRequested);
                }
                self.close_drag_dy = 0.0;
            }
            PanUpdate::Cancelled => self.close_drag_dy = 0.0,
        }
        Ok(())
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        kit.set_interactive(self.scrim, ctx.state.calibration().menu() != CalibrationMenu::Closed)?;
        self.sheet.sync(kit, ctx)?;
        self.card.sync(kit, ctx)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>) {
        self.sheet.advance(ctx);
        self.card.advance(ctx);
    }

    /// Рисует меню в два слоя поверх `layers`: шторка, затем карточка.
    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(host) = RootHost::of(ctx, visuals, self.root, self.root_center) else {
            return Ok(());
        };
        let mut sheet_layers = DrawLayers::default();
        self.sheet.draw(
            ctx,
            (visuals, &host),
            &mut sheet_layers,
            texts,
            self.card.cover_progress(ctx.now),
        )?;
        layers.stack_on_top(sheet_layers);
        let mut card_layers = DrawLayers::default();
        self.card.draw(ctx, (visuals, &host), &mut card_layers, texts)?;
        layers.stack_on_top(card_layers);
        Ok(())
    }
}
