//! Геометрия виджетов в pt нашего экрана: числа эталонного iPhone из конфига + привязка к низу (решение 82).
//!
//! Единственное место, где координаты iPhone превращаются в прямоугольники нашего экрана; виджеты берут
//! рамки отсюда, а не считают сами. Кадр видоискателя приходит из [`ScreenLayout`] (решение 93).

use ui_core::geometry::{Point, Rect, Size};
use ui_kit::tokens::UiKitTokens;

use crate::layout::ScreenLayout;
use crate::settings::AspectRatio;
use crate::widgets_config::{RoundButtonLayout, WidgetLayoutConfig};
use crate::zoom_config::ZoomConfig;

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
    zoom: ZoomConfig,
    shutter_diameter_pt: f64,
    /// Длина риски линейки (токен `ruler.tick_length_pt`): по ней зона шкалы ставится на низ рисок.
    pub(super) ruler_tick_length_pt: f64,
    /// Другая «поверхность» вместо экрана: глазной прямоугольник раздела калибровки в системе его корня. Рамки,
    /// привязанные к низу и середине экрана (шторка, карточка), тогда встают у низа и середины этого
    /// прямоугольника. `None` — обычный экран.
    screen_override: Option<Rect>,
}

impl ScreenGeometry {
    pub(crate) fn new(
        layout: &ScreenLayout,
        config: &WidgetLayoutConfig,
        zoom: &ZoomConfig,
        tokens: &UiKitTokens,
    ) -> Self {
        Self {
            layout: *layout,
            config: config.clone(),
            zoom: zoom.clone(),
            shutter_diameter_pt: tokens.shutter.outer_diameter_pt,
            ruler_tick_length_pt: tokens.ruler.tick_length_pt,
            screen_override: None,
        }
    }

    /// Та же геометрия, но «экран» — прямоугольник `screen` (глазной прямоугольник раздела калибровки, в системе
    /// корня раздела). Шторка и карточка раздела строятся теми же числами, что в PHOTO, но у его низа и середины.
    pub(crate) fn in_screen(&self, screen: Rect) -> Self {
        Self {
            screen_override: Some(screen),
            ..self.clone()
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
        let screen = self.screen();
        let bottom = screen.max_y() - bottom_inset_pt;
        Rect::from_xywh(
            screen.min_x() + (screen.size.width - width_pt) * 0.5,
            bottom - height_pt,
            width_pt,
            height_pt,
        )
    }

    pub(crate) fn screen(&self) -> Rect {
        self.screen_override
            .unwrap_or_else(|| Rect::new(Point::new(0.0, 0.0), self.layout.screen_size()))
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
        self.menu_item_in(self.sheet(), slot)
    }

    /// Круг пункта `slot` в шторке `sheet_rect` (сетка центрирована по её ширине).
    pub(crate) fn menu_item_in(&self, sheet_rect: Rect, slot: usize) -> Rect {
        let sheet = &self.config.sheet;
        let rect = sheet_rect;
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
        self.sheet_handle_in(self.sheet())
    }

    /// «Ручка» шторки `sheet_rect`.
    pub(crate) fn sheet_handle_in(&self, sheet_rect: Rect) -> Rect {
        let sheet = &self.config.sheet;
        let rect = sheet_rect;
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

    // ---- зум (сессия 13) ----

    /// На сколько pt координаты эталонного iPhone сдвинуты вниз (привязка к низу).
    pub(crate) fn bottom_shift_pt(&self) -> f64 {
        self.layout.bottom_anchor_shift_pt()
    }

    pub(crate) fn zoom_config(&self) -> &ZoomConfig {
        &self.zoom
    }

    /// Зона ленты зума: охватывает все положения кнопок при любой выбранной (лента едет под выбранной).
    pub(crate) fn zoom_strip(&self) -> Rect {
        let strip = &self.zoom.strip;
        let reach = f64::from(strip.max_buttons.saturating_sub(1)) * strip.pitch_pt + strip.touch_size_pt * 0.5;
        self.centered(strip.selected_x_pt, strip.center_y_pt, reach * 2.0, strip.touch_size_pt)
    }

    /// Рамка кнопки `slot`, когда выбрана кнопка `selected`: выбранная всегда на `selected_x_pt`.
    pub(crate) fn zoom_button(&self, slot: usize, selected: usize) -> Rect {
        let strip = &self.zoom.strip;
        let offset = slot as f64 - selected as f64;
        self.centered(
            strip.selected_x_pt + offset * strip.pitch_pt,
            strip.center_y_pt,
            strip.touch_size_pt,
            strip.touch_size_pt,
        )
    }

    /// Зона колеса: от верха кольца рисок до низа экрана, на всю ширину (диск шире экрана и обрезан им).
    pub(crate) fn zoom_wheel_zone(&self) -> Rect {
        let screen = self.screen();
        let top = self.zoom.wheel.touch_top_pt + self.layout.bottom_anchor_shift_pt();
        Rect::from_xywh(0.0, top, screen.size.width, (screen.size.height - top).max(1.0))
    }

    /// Центр диска колеса в абсолютных координатах экрана.
    pub(crate) fn zoom_wheel_center(&self) -> Point {
        Point::new(
            self.zoom.wheel.center_x_pt,
            self.zoom.wheel.center_y_pt + self.layout.bottom_anchor_shift_pt(),
        )
    }

    /// Кнопка переворота камеры.
    pub(crate) fn flip_button(&self) -> Rect {
        self.round(self.zoom.flip_button)
    }
}
