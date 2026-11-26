//! Нижние карточки раздела калибровки (решения 94, 199, 201, 283, 292): карточка пресетов (кнопка меню) и карточка
//! белого (капсула в полоске прибора) под ОДНОЙ модальной завесой.
//!
//! Всё лежит под повёрнутым корнем раздела (глазное пространство, `SectionSpace`) и строится теми же средствами,
//! что меню Controls экрана PHOTO: карточка — `PanelMorph`, боковые кнопки — `SideButton`, ряды таблеток —
//! `ChipRow`, числа — `widgets.card`. Геометрия карточки берётся у `ScreenGeometry::in_screen` на глазном
//! прямоугольнике: у его низа, а не экрана.
//!
//! # Владение и инварианты
//! * Какая карточка открыта, владеет `CalibrationState::card`; виджеты выравниваются по нему в `sync` (без эха).
//! * Пока открыта любая карточка, под ней лежат ручки, прибор, капсула режимов и обе кнопки нижнего ряда: «скрим» на
//!   весь корень (первый узел, интерактивен только при открытой карточке) ловит любой тап мимо карточки и ТОЛЬКО
//!   закрывает её (решение 281: открытое меню модальное, тап по капсуле режимов не меняет режим; то же для капсулы
//!   белого: повторный тап по ней — это тап по скриму). Карточка лежит над скримом.
//! * Нижний ряд (CLEAN, капсула режимов, кнопка меню) не рисуется, пока карточка видна ([`SectionCards::covers_bottom_row`],
//!   решение 292), а касания до него не доходят из-за скрима.
//! * Закрывает меню также свайп вниз (в глазных осях) по карточке; закрытие по смещению, а не по скорости, чтобы
//!   медленный свайп тоже закрывал.
//! * Рисуется меню поверх всего раздела (`DrawLayers::stack_on_top`).

mod card;
mod card_morph;
mod white_card;

use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureEvent, GestureState, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_kit::pan::PanUpdate;

use self::card::MenuCard;
use self::white_card::SectionWhiteCard;
use super::host::RootHost;
use super::space::SectionSpace;
use crate::action::ScreenAction;
use crate::calibration::SectionCard;
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Завеса и обе нижние карточки раздела.
pub(super) struct SectionCards {
    space: SectionSpace,
    root: NodeId,
    root_center: Point,
    scrim: NodeId,
    card: MenuCard,
    /// В куче: карточка несёт две шкалы и ряд кнопок, а `Stage` лежит на стеке целиком (тесты со многими экранами).
    white: Box<SectionWhiteCard>,
    /// Последнее смещение пальца по глазной вертикали за идущий свайп, pt (вниз — положительное).
    close_drag_dy: f64,
}

impl SectionCards {
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
        let white = Box::new(SectionWhiteCard::build(kit, ctx, (&eye, space), root)?);
        Ok(Self {
            space: *space,
            root,
            root_center: root_frame.center(),
            scrim,
            card,
            white,
            close_drag_dy: 0.0,
        })
    }

    /// Верх открытой карточки в системе корня, pt; `None` — карточек нет. Плашка проблемы встаёт над этой линией, а
    /// не поверх карточки.
    pub(super) fn open_top(&self, card: SectionCard) -> Option<f64> {
        match card {
            SectionCard::Closed => None,
            SectionCard::Presets => Some(self.card.rect().min_y()),
            SectionCard::White => Some(self.white.rect().min_y()),
        }
    }

    /// Лежит ли какая-нибудь карточка над нижним рядом (в том числе пока растворяется): ряд не рисуется.
    pub(super) fn covers_bottom_row(&self, ctx: &Ctx<'_>) -> bool {
        ctx.state.calibration().card() != SectionCard::Closed || self.card.is_visible(ctx) || self.white.is_visible(ctx)
    }

    /// Нужны ли кадры из-за карточки белого (мерцание «Phone», тряска).
    pub(super) fn is_animating(&self, now: ui_core::time::MonotonicNanos) -> bool {
        self.white.is_animating(now)
    }

    /// Сколько значений шкал белого не стали белым (диагностика).
    pub(super) fn invalid_white_values(&self) -> u64 {
        self.white.invalid_values()
    }

    /// Имена пресетов, не поместившиеся в ряд карточки (диагностика).
    pub(super) fn overflowed_chips(&self) -> usize {
        self.card.overflowed_chips()
    }

    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        ctx: &Ctx<'_>,
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
            _ => Ok(self.card.handle_gesture(kit, event, out)? || self.white.handle_gesture(kit, event, ctx, out)?),
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
        kit.set_interactive(self.scrim, ctx.state.calibration().card() != SectionCard::Closed)?;
        self.card.sync(kit, ctx)?;
        self.white.sync(kit, ctx)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.card.advance(ctx);
        self.white.advance(ctx, out);
    }

    /// Рисует карточки поверх всего раздела.
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
        self.white.draw(ctx, (visuals, &host), &mut card_layers, texts)?;
        layers.stack_on_top(card_layers);
        Ok(())
    }
}
