//! Рамки нижней карточки инструмента и поднятого ряда затвора (решение 103).
//!
//! Продолжение [`ScreenGeometry`]: числа берутся из [`crate::widgets_config::CardLayout`], привязку к низу
//! экрана делает базовая геометрия. Все вертикальные отступы карточки отсчитываются от её верха, поэтому
//! при другой высоте экрана (4:3, 16:9) карточка и её содержимое переезжают вместе.

use ui_core::geometry::{Point, Rect, Size};

use super::geometry::ScreenGeometry;
use crate::widgets_config::CardPoint;

impl ScreenGeometry {
    /// Рамка карточки: у низа экрана, по центру по x.
    pub(crate) fn card(&self) -> Rect {
        let card = &self.config().card;
        self.bottom_anchored(card.width_pt, card.height_pt, card.bottom_inset_pt)
    }

    /// Центр точки карточки в абсолютных pt: x — от середины карточки, y — от её верха.
    fn card_point(&self, point: CardPoint) -> Point {
        let card = self.card();
        Point::new(
            card.center().x + point.offset_from_center_x_pt,
            card.min_y() + point.center_from_top_pt,
        )
    }

    /// Круглая боковая кнопка Ø `button_diameter_pt` вокруг точки карточки.
    fn card_button(&self, point: CardPoint) -> Rect {
        let diameter = self.config().card.button_diameter_pt;
        Rect::centered_at(self.card_point(point), Size::new(diameter, diameter))
    }

    /// Левая кнопка: «‹» (назад в меню) или «×» (закрыть).
    pub(crate) fn card_left_button(&self) -> Rect {
        self.card_button(self.config().card.left_button)
    }

    /// Правая кнопка: «↻» сброс (только Exposure).
    pub(crate) fn card_right_button(&self) -> Rect {
        self.card_button(self.config().card.right_button)
    }

    /// Центр заголовка карточки (по центру карточки по x).
    pub(crate) fn card_title_center(&self) -> Point {
        let card = &self.config().card;
        Point::new(
            self.card().center().x,
            self.card().min_y() + card.title_center_from_top_pt,
        )
    }

    /// Центр крупного значения.
    pub(crate) fn card_value_center(&self) -> Point {
        let card = &self.config().card;
        Point::new(
            self.card().center().x,
            self.card().min_y() + card.value_center_from_top_pt,
        )
    }

    /// Зона варианта `index` из `count`: ряд центрирован по `options_center` (середина карточки плюс смещение), шаг — конфиг.
    pub(crate) fn card_option_slot(&self, index: usize, count: usize) -> Rect {
        let card = &self.config().card;
        let center = self.card_point(card.options_center);
        let offset_from_middle = index as f64 - (count.saturating_sub(1)) as f64 * 0.5;
        Rect::centered_at(
            Point::new(center.x + offset_from_middle * card.option_pitch_pt, center.y),
            Size::new(card.option_pitch_pt, card.option_touch_height_pt),
        )
    }

    /// Ряд таблеток на месте ряда вариантов инструментов: центр и ширина окна (по бокам оставлено `side_inset_pt`).
    pub(crate) fn card_chip_row(&self, side_inset_pt: f64) -> (Point, f64) {
        let card = &self.config().card;
        (
            self.card_point(card.options_center),
            (card.width_pt - 2.0 * side_inset_pt).max(0.0),
        )
    }

    /// Зона шкалы Exposure: вся ширина карточки; середина по высоте — середина рисок (их низ — `T + 129`).
    pub(crate) fn card_ruler(&self) -> Rect {
        let card = &self.config().card;
        let rect = self.card();
        let ticks_center_y = rect.min_y() + card.ruler_ticks_bottom_from_top_pt - self.ruler_tick_length_pt * 0.5;
        Rect::centered_at(
            Point::new(rect.center().x, ticks_center_y),
            Size::new(rect.size.width, card.ruler_touch_height_pt),
        )
    }
}
