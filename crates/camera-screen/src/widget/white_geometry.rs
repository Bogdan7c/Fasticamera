//! Рамки карточки белого (WB28) и кнопки белого у затвора (WB35): продолжение [`ScreenGeometry`]. Числа — из `white_layout.rs`, привязка к низу —
//! базовая геометрия. Все вертикали карточки отсчитываются от её верха, как у обычной карточки.

use ui_core::geometry::{Point, Rect, Size};

use super::geometry::ScreenGeometry;
use crate::tools::ToolId;
use crate::white::WhiteAxis;
use crate::white_config::WhiteButtonSide;

impl ScreenGeometry {
    /// Рамка кнопки белого в ряду затвора (WB35): круг диаметром кнопки меню, центр на высоте центра затвора, по x —
    /// на смещении раскладки от центра затвора, справа или слева. Это рамка в покое: когда затвор поднят над
    /// карточкой, кнопка едет вместе с ним визуальным сдвигом (`shutter_row.rs`), а рамка раскладки не меняется.
    pub(crate) fn white_button(&self, side: WhiteButtonSide) -> Rect {
        let bottom = &self.config().bottom;
        let center_x_pt = self.config().white_button.center_x_pt(bottom.shutter_center_x_pt, side);
        let diameter_pt = bottom.menu_button.diameter_pt;
        self.centered(center_x_pt, bottom.shutter_center_y_pt, diameter_pt, diameter_pt)
    }

    /// Рамка белой карточки: ширина, радиус и низ как у обычной, высота на `extra_height_pt` больше.
    pub(crate) fn white_card(&self) -> Rect {
        let card = &self.config().card;
        let white = &self.config().white_card;
        self.bottom_anchored(card.width_pt, white.card_height_pt(card), card.bottom_inset_pt)
    }

    /// Точка белой карточки: x — от середины, y — от верха карточки (абсолютные pt).
    fn white_point(&self, offset_from_center_x_pt: f64, center_from_top_pt: f64) -> Point {
        let rect = self.white_card();
        Point::new(
            rect.center().x + offset_from_center_x_pt,
            rect.min_y() + center_from_top_pt,
        )
    }

    /// «×» белой карточки: тот же диск и то же место по x/y от верха, что у «×» обычной карточки со значка.
    pub(crate) fn white_close_button(&self) -> Rect {
        let card = &self.config().card;
        let diameter = card.button_diameter_pt;
        let center = self.white_point(
            card.left_button.offset_from_center_x_pt,
            card.left_button.center_from_top_pt,
        );
        Rect::centered_at(center, Size::new(diameter, diameter))
    }

    /// Зона числа оси: Кельвины слева от середины, оттенок справа.
    pub(crate) fn white_axis_button(&self, axis: WhiteAxis) -> Rect {
        let white = &self.config().white_card;
        let side = match axis {
            WhiteAxis::Temperature => -1.0,
            WhiteAxis::Tint => 1.0,
        };
        let center = self.white_point(side * white.axis_center_offset_pt, white.axis_row_center_from_top_pt);
        Rect::centered_at(center, Size::new(white.axis_touch_width_pt, white.axis_touch_height_pt))
    }

    /// Зона кнопки `index` из `count` в ряду «Phone» + кнопки света: ряд центрирован по карточке, шаг — конфиг.
    pub(crate) fn white_light_slot(&self, index: usize, count: usize) -> Rect {
        let white = &self.config().white_card;
        let offset_from_middle = index as f64 - count.saturating_sub(1) as f64 * 0.5;
        let center = self.white_point(
            offset_from_middle * white.light_pitch_pt,
            white.lights_row_center_from_top_pt,
        );
        Rect::centered_at(center, Size::new(white.light_pitch_pt, white.lights_touch_height_pt))
    }

    /// Зона шкалы: вся ширина карточки; верх и низ — конфиг (ВСЯ выше полосы системного жеста, это проверяет конфиг).
    pub(crate) fn white_ruler(&self) -> Rect {
        let white = &self.config().white_card;
        let width = self.config().card.width_pt;
        let center = self.white_point(0.0, white.ruler_center_from_top_pt);
        Rect::centered_at(center, Size::new(width, white.ruler_touch_height_pt))
    }

    /// На сколько pt белая карточка выше обычной (0 для любой другой карточки).
    pub(crate) fn card_extra_height_pt(&self, tool: Option<ToolId>) -> f64 {
        if tool == Some(ToolId::White) {
            self.config().white_card.extra_height_pt
        } else {
            0.0
        }
    }

    /// На сколько pt поднят ряд затвора над своим обычным местом, пока открыта карточка `tool`:
    /// обычные 100 pt, у белой карточки ещё `raised_shutter_extra_pt`.
    pub(crate) fn raised_shutter_offset_pt(&self, tool: Option<ToolId>) -> f64 {
        let base = self.config().card.raised_shutter_offset_pt;
        if tool == Some(ToolId::White) {
            base + self.config().white_card.raised_shutter_extra_pt
        } else {
            base
        }
    }
}
