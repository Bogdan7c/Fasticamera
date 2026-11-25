//! Публичный список целей касаний белого для стендов и smoke-проверок: где (в pt экрана) нажимать пальцем, чтобы
//! попасть в кнопку белого, «×», числа осей, «Phone», кнопки света и шкалу.
//!
//! Цели считает та же геометрия ([`ScreenGeometry`]), по которой строятся узлы сцены, поэтому формул здесь нет
//! и список не может разойтись с тем, что реально ловит касание. Цели зависят от состояния: кнопка белого стоит
//! в покое у затвора и поднята на высоту подъёма затвора, пока открыта карточка (положение после конца анимации);
//! цели карточки — только пока открыта карточка белого.
//!
//! Там же — запрос «нужны ли кадры»: [`CameraScreen::white_attention_animating`].

use ui_core::geometry::{Point, Rect};

use super::CameraScreen;
use crate::state::{PanelState, ShutterVisibility};
use crate::tools::ToolId;
use crate::white::WhiteAxis;
use crate::widget::geometry::ScreenGeometry;
use crate::widget::tool_card_content::open_card_tool;

/// Цели касаний белого, pt экрана (портретные оси surface, как у касаний `ScreenTouch`).
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteTouchTargets {
    /// Зона касания кнопки белого у затвора, как её видит палец сейчас (с подъёмом, если открыта карточка).
    pub button: Rect,
    /// Цели открытой карточки белого; `None` — карточка белого не открыта.
    pub card: Option<WhiteCardTargets>,
}

/// Цели открытой карточки белого.
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteCardTargets {
    /// Центр «×».
    pub close: Point,
    /// Центр числа Кельвинов.
    pub temperature_number: Point,
    /// Центр числа оттенка.
    pub tint_number: Point,
    /// Центр кнопки «Phone».
    pub phone_button: Point,
    /// Центры кнопок света в порядке конфига.
    pub light_buttons: Vec<Point>,
    /// Зона касания шкалы (вся ширина карточки).
    pub ruler_zone: Rect,
}

impl CameraScreen {
    /// Цели касаний белого по текущему состоянию; `None` — раскладки ещё нет (не было размера surface).
    pub fn white_touch_targets(&self) -> Option<WhiteTouchTargets> {
        let layout = self.state.layout()?;
        let geometry = ScreenGeometry::new(layout, &self.config.widgets, &self.config.zoom, &self.tokens);
        let button = {
            let rest = geometry.white_button(self.config.white.button_side);
            let raise_pt = match self.state.shutter_visibility() {
                ShutterVisibility::RaisedAboveCard => geometry.raised_shutter_offset_pt(open_card_tool(&self.state)),
                ShutterVisibility::Visible | ShutterVisibility::HiddenUnderDrawer => 0.0,
            };
            Rect::from_xywh(rest.min_x(), rest.min_y() - raise_pt, rest.size.width, rest.size.height)
        };
        let card_open = self.state.panel() == PanelState::Submenu(ToolId::White);
        let card = card_open.then(|| {
            let buttons = 1 + self.config.white.lights.len();
            let slot = |index: usize| geometry.white_light_slot(index, buttons).center();
            WhiteCardTargets {
                close: geometry.white_close_button().center(),
                temperature_number: geometry.white_axis_button(WhiteAxis::Temperature).center(),
                tint_number: geometry.white_axis_button(WhiteAxis::Tint).center(),
                phone_button: slot(0),
                light_buttons: (1..buttons).map(slot).collect(),
                ruler_zone: geometry.white_ruler(),
            }
        });
        Some(WhiteTouchTargets { button, card })
    }

    /// Нужны ли ещё кадры из-за кнопок белого: идёт мерцание ожидания ответа источника или тряска отказа (кнопка
    /// у затвора и «Phone» на карточке). Рендер строит кадры на каждом показе, пока камера жива; запрос нужен
    /// стендам и приложению, которые рисуют по требованию. `false` до первого кадра.
    pub fn white_attention_animating(&self) -> bool {
        self.stage
            .as_ref()
            .is_some_and(|stage| stage.white_attention_animating(self.clock.last))
    }
}

#[cfg(test)]
impl CameraScreen {
    /// Положение шкалы оси в единицах положения (майреды со знаком у Кельвинов, единицы у оттенка), без округления.
    pub(crate) fn white_driven(&self) -> bool {
        self.stage
            .as_ref()
            .is_some_and(|stage| stage.card().white().ruler().driven())
    }

    pub(crate) fn white_scale_position(&self, axis: WhiteAxis) -> Option<f64> {
        Some(
            self.stage
                .as_ref()?
                .card()
                .white()
                .ruler()
                .position_of(axis, self.clock.last),
        )
    }
}
