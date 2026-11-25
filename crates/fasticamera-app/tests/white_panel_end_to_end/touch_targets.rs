//! Где на экране лежат цели касаний (pt), посчитанные ТЕМИ ЖЕ числами конфига и раскладки, которыми экран строит
//! свои рамки (`ScreenGeometry` в `camera-screen` закрыта `pub(crate)`, поэтому формулы повторены здесь по
//! публичным данным: конфиг виджетов + `ScreenLayout`). Если экран изменит формулу, тесты с касаниями упадут: жест не
//! попадёт в цель, и это громкий сигнал, а не тихая ошибка.

use camera_screen::{ScreenConfig, ScreenState};
use ui_core::geometry::Point;
use ui_kit::tokens::UiKitTokens;

/// Сколько значков стоит в ряду над кадром у экрана по умолчанию в режиме фото (вспышка, RAW, EV): значок EV —
/// последний из них.
pub const VISIBLE_STRIP_ICONS: usize = 3;

/// Центр кадра видоискателя: сюда тапают, чтобы навестись на точку.
pub fn viewfinder_center(state: &ScreenState) -> Point {
    let layout = state.layout().expect("раскладка экрана построена");
    layout.frame_rect(state.settings().aspect).center()
}

/// Центр значка EV над кадром (последний в ряду из [`VISIBLE_STRIP_ICONS`]).
pub fn ev_strip_icon(state: &ScreenState) -> Point {
    let layout = state.layout().expect("раскладка экрана построена");
    let strip = &ScreenConfig::default().widgets.strip;
    let index = VISIBLE_STRIP_ICONS - 1;
    let offset_from_middle = index as f64 - (VISIBLE_STRIP_ICONS - 1) as f64 * 0.5;
    Point::new(
        strip.center_x_pt + offset_from_middle * strip.pitch_pt,
        strip.center_y_pt + layout.bottom_anchor_shift_pt(),
    )
}

/// Центр шкалы карточки Exposure (риски): вдоль неё ведут палец, чтобы поменять EV.
pub fn exposure_card_ruler(state: &ScreenState) -> Point {
    let layout = state.layout().expect("раскладка экрана построена");
    let card = &ScreenConfig::default().widgets.card;
    let tick_length_pt = UiKitTokens::default().ruler.tick_length_pt;
    let screen = layout.screen_size();
    let card_top = screen.height - card.bottom_inset_pt - card.height_pt;
    Point::new(
        screen.width * 0.5,
        card_top + card.ruler_ticks_bottom_from_top_pt - tick_length_pt * 0.5,
    )
}
