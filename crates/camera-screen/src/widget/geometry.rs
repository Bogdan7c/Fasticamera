//! Геометрия виджетов в pt нашего экрана: числа эталонного iPhone из конфига + привязка к низу (решение 82).
//!
//! Единственное место, где координаты iPhone превращаются в прямоугольники нашего экрана; виджеты берут
//! рамки отсюда, а не считают сами. Кадр видоискателя приходит из [`ScreenLayout`] (решение 93).

use ui_core::geometry::{Point, Rect, Size};
use ui_kit::tokens::UiKitTokens;

use crate::layout::ScreenLayout;
use crate::settings::AspectRatio;
use crate::widgets_config::{RoundButtonLayout, WidgetLayoutConfig};

/// Сколько значков над кадром может стоять одновременно (вспышка, RAW, таймер, EV).
pub(crate) const MAX_STRIP_ICONS: usize = 4;
/// Сколько пунктов вмещает сетка меню (3 × 3).
pub(crate) const MAX_MENU_ITEMS: usize = 9;
/// Сколько вариантов вмещает карточка (у таймера их четыре — максимум в v1).
pub(crate) const MAX_SUBMENU_OPTIONS: usize = 4;

/// Рамки всех виджетов для одной раскладки.
#[derive(Debug, Clone)]
pub(crate) struct ScreenGeometry {
    layout: ScreenLayout,
    config: WidgetLayoutConfig,
    shutter_diameter_pt: f64,
    /// Длина риски линейки (токен `ruler.tick_length_pt`): по ней зона шкалы ставится на низ рисок.
    pub(super) ruler_tick_length_pt: f64,
}

impl ScreenGeometry {
    pub(crate) fn new(layout: &ScreenLayout, config: &WidgetLayoutConfig, tokens: &UiKitTokens) -> Self {
        Self {
            layout: *layout,
            config: config.clone(),
            shutter_diameter_pt: tokens.shutter.outer_diameter_pt,
            ruler_tick_length_pt: tokens.ruler.tick_length_pt,
        }
    }

    pub(crate) fn config(&self) -> &WidgetLayoutConfig {
        &self.config
    }

    /// Рамка центрированного элемента; `center_y_reference_pt` — y эталонного экрана (сдвиг к низу добавляется тут).
    pub(super) fn centered(&self, center_x_pt: f64, center_y_reference_pt: f64, width_pt: f64, height_pt: f64) -> Rect {
        let center = Point::new(
            center_x_pt,
            center_y_reference_pt + self.layout.bottom_anchor_shift_pt(),
        );
        Rect::centered_at(center, Size::new(width_pt, height_pt))
    }

    fn round(&self, button: RoundButtonLayout) -> Rect {
        self.centered(
            button.center_x_pt,
            button.center_y_pt,
            button.diameter_pt,
            button.diameter_pt,
        )
    }

    /// Рамка у низа экрана: центр по x экрана, низ на `bottom_inset_pt` выше низа.
    pub(super) fn bottom_anchored(&self, width_pt: f64, height_pt: f64, bottom_inset_pt: f64) -> Rect {
        let screen = self.layout.screen_size();
        let bottom = screen.height - bottom_inset_pt;
        Rect::from_xywh((screen.width - width_pt) * 0.5, bottom - height_pt, width_pt, height_pt)
    }

    pub(crate) fn screen(&self) -> Rect {
        Rect::new(Point::new(0.0, 0.0), self.layout.screen_size())
    }

    pub(crate) fn frame(&self, aspect: AspectRatio) -> Rect {
        self.layout.frame_rect(aspect)
    }

    pub(crate) fn shutter(&self) -> Rect {
        let bottom = &self.config.bottom;
        self.centered(
            bottom.shutter_center_x_pt,
            bottom.shutter_center_y_pt,
            self.shutter_diameter_pt,
            self.shutter_diameter_pt,
        )
    }

    pub(crate) fn menu_button(&self) -> Rect {
        self.round(self.config.bottom.menu_button)
    }

    pub(crate) fn thumbnail(&self) -> Rect {
        self.round(self.config.bottom.thumbnail)
    }

    pub(crate) fn capsule(&self) -> Rect {
        let bottom = &self.config.bottom;
        self.centered(
            bottom.capsule_center_x_pt,
            bottom.capsule_center_y_pt,
            bottom.capsule_width_pt,
            bottom.capsule_height_pt,
        )
    }

    /// Зона касания значка `index` из `count` видимых: ряд центрирован, шаг — конфиг.
    pub(crate) fn strip_icon(&self, index: usize, count: usize) -> Rect {
        let strip = &self.config.strip;
        let offset_from_middle = index as f64 - (count.saturating_sub(1)) as f64 * 0.5;
        self.centered(
            strip.center_x_pt + offset_from_middle * strip.pitch_pt,
            strip.center_y_pt,
            strip.touch_size_pt,
            strip.touch_size_pt,
        )
    }

    pub(crate) fn sheet(&self) -> Rect {
        let sheet = &self.config.sheet;
        self.bottom_anchored(sheet.width_pt, sheet.height_pt, sheet.bottom_inset_pt)
    }

    /// Круг пункта сетки меню (абсолютная рамка, pt): сетка центрирована по ширине шторки.
    pub(crate) fn menu_item(&self, slot: usize) -> Rect {
        let sheet = &self.config.sheet;
        let rect = self.sheet();
        let columns = usize::from(sheet.columns.max(1));
        let (column, row) = (slot % columns, slot / columns);
        let grid_width = sheet.column_pitch_pt * (columns - 1) as f64 + sheet.item_diameter_pt;
        let left = rect.min_x() + (rect.size.width - grid_width) * 0.5;
        // Центр первого ряда задан от верха шторки (`first_row_center_from_top_pt`), рамка круга — на радиус выше.
        let first_row_top = rect.min_y() + sheet.first_row_center_from_top_pt - sheet.item_diameter_pt * 0.5;
        Rect::from_xywh(
            left + column as f64 * sheet.column_pitch_pt,
            first_row_top + row as f64 * sheet.row_pitch_pt,
            sheet.item_diameter_pt,
            sheet.item_diameter_pt,
        )
    }

    /// «Ручка» шторки: полоска по центру шторки на `handle_top_inset_pt` ниже её верха.
    pub(crate) fn sheet_handle(&self) -> Rect {
        let sheet = &self.config.sheet;
        let rect = self.sheet();
        Rect::from_xywh(
            rect.center().x - sheet.handle_width_pt * 0.5,
            rect.min_y() + sheet.handle_top_inset_pt,
            sheet.handle_width_pt,
            sheet.handle_height_pt,
        )
    }

    /// Пилюля выбранного режима: по центру капсулы.
    pub(crate) fn mode_pill(&self) -> Rect {
        let bottom = &self.config.bottom;
        Rect::centered_at(
            self.capsule().center(),
            Size::new(bottom.mode_pill_width_pt, bottom.mode_pill_height_pt),
        )
    }

    /// Центр плашки AE/AF LOCK: по центру кадра, на отступе от его верха.
    pub(crate) fn lock_plate_center(&self, aspect: AspectRatio, plate_height_pt: f64) -> Point {
        let frame = self.frame(aspect);
        Point::new(
            frame.center().x,
            frame.min_y() + self.config.focus.lock_plate_top_inset_pt + plate_height_pt * 0.5,
        )
    }
}
