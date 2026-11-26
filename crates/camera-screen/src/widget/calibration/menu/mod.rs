//! Меню «шесть точек» раздела калибровки (решения 94, 199, 201, 283): карточка пресетов, открывается кнопкой меню сразу.
//!
//! Всё лежит под повёрнутым корнем раздела (глазное пространство, `SectionSpace`) и строится теми же средствами,
//! что меню Controls экрана PHOTO: карточка — `PanelMorph`, боковые кнопки — `SideButton`, ряды таблеток —
//! `ChipRow`, числа — `widgets.card`. Геометрия карточки берётся у `ScreenGeometry::in_screen` на глазном
//! прямоугольнике: у его низа, а не экрана.
//!
//! # Владение и инварианты
//! * Открыто ли меню, владеет `CalibrationState::menu`; виджеты выравниваются по нему в `sync` (без эха).
//! * Пока меню открыто, под ним лежат ручки, прибор, капсула режимов и обе кнопки нижнего ряда: «скрим» на весь корень
//!   (первый узел меню, интерактивен только при открытом меню) ловит любой тап мимо карточки и ТОЛЬКО закрывает меню
//!   (решение 281: открытое меню модальное, тап по капсуле не меняет режим). Карточка лежит над скримом.
//! * Закрывает меню также свайп вниз (в глазных осях) по карточке; закрытие по смещению, а не по скорости, чтобы
//!   медленный свайп тоже закрывал.
//! * Рисуется меню поверх всего раздела (`DrawLayers::stack_on_top`).

mod card;

use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureEvent, GestureState, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_kit::pan::PanUpdate;

use self::card::MenuCard;
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
        let card = MenuCard::build(kit, ctx, (&eye, space), (root, root_frame))?;
        Ok(Self {
            space: *space,
            root,
            root_center: root_frame.center(),
            scrim,
            card,
            close_drag_dy: 0.0,
        })
    }

    /// Верх открытой карточки в системе корня, pt; `None` — меню закрыто. Плашка проблемы встаёт над этой линией, а
    /// не поверх меню.
    pub(super) fn open_top(&self, menu: CalibrationMenu) -> Option<f64> {
        match menu {
            CalibrationMenu::Closed => None,
            CalibrationMenu::Presets => Some(self.card.rect().min_y()),
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
            _ => self.card.handle_gesture(kit, event, out),
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
        self.card.sync(kit, ctx)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>) {
        self.card.advance(ctx);
    }

    /// Рисует карточку поверх всего раздела.
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
        let mut card_layers = DrawLayers::default();
        self.card.draw(ctx, (visuals, &host), &mut card_layers, texts)?;
        layers.stack_on_top(card_layers);
        Ok(())
    }
}
