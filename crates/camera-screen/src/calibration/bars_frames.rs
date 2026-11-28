//! Геометрия вида BARS (решение 343): ОДНА чистая функция от рамки содержимого колонки и конфига.
//!
//! Раскладка сверху вниз: ряд вкладок LIFT · GAMMA · GAIN · OFFSET и три строки-шкалы R, G, B. Строки названия зоны и
//! «↻ всей зоны» нет: в альбоме (контент 272 pt) вкладки и три шкалы занимают 248 pt, а лишние 30 pt уже не помещаются.
//! Раскладка ОДНА для обеих ориентаций: колонка уже повёрнута вместе с разделом, функция работает в системе колонки.
//!
//! Функцию читают и виджет, и фасад экрана (запрос «где шкала канала на поверхности»), и тесты: рамки не считаются
//! дважды, сквозной тест не может разойтись с тем, что рисует экран.

use look_contract::PrimaryChannel;
use thiserror::Error;
use ui_core::geometry::{Point, Rect};

use super::widgets_config::CalibrationWidgetsConfig;

/// Почему вид BARS нельзя разложить.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum BarsFramesError {
    /// Вкладки и три строки не помещаются в окно без прокрутки.
    #[error("вид BARS не помещается: нужно {required_pt} pt, доступно {available_pt} pt")]
    NoRoom { required_pt: f64, available_pt: f64 },
}

/// Рамки вида BARS: вкладки и три строки по порядку `PrimaryChannel::ALL` (R, G, B).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BarsFrames {
    /// Ряд вкладок зон.
    pub tabs: Rect,
    /// Строки-шкалы каналов R, G, B сверху вниз.
    pub rows: [Rect; 3],
}

impl BarsFrames {
    /// Раскладывает `content` (внутренняя ширина колонки × высота окна). Не помещается — ошибка, а не обрезка.
    pub(crate) fn of(content: Rect, config: &CalibrationWidgetsConfig) -> Result<Self, BarsFramesError> {
        // Над первой строкой её «↻» забирает полосу `row_reset_reach_above_pt` (решение 279): вкладки кончаются выше.
        let gap = config.wheel_block_gap_pt.max(config.row_reset_reach_above_pt);
        let rows_height = PrimaryChannel::ALL.len() as f64 * config.row_height_pt;
        // Снизу — отступ панели, как у списка шкал и вида колёс.
        let required_pt = config.wheel_tabs_height_pt + gap + rows_height + config.panel_padding_pt;
        if required_pt > content.size.height + 1e-9 {
            return Err(BarsFramesError::NoRoom {
                required_pt,
                available_pt: content.size.height,
            });
        }
        let tabs = Rect::from_xywh(
            content.min_x(),
            content.min_y(),
            content.size.width,
            config.wheel_tabs_height_pt,
        );
        let first_row_top = tabs.max_y() + gap;
        let rows = std::array::from_fn(|index| {
            Rect::from_xywh(
                content.min_x(),
                first_row_top + index as f64 * config.row_height_pt,
                content.size.width,
                config.row_height_pt,
            )
        });
        Ok(Self { tabs, rows })
    }
}

/// Где на поверхности стоит шкала канала (pt): ответ запроса фасада для сквозных тестов.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarsRulerPlacement {
    /// Рамка строки канала на поверхности (оси экрана, начало в левом верхнем углу).
    pub row: Rect,
    /// Центр линейки строки (нижняя часть под заголовком) на поверхности: сюда ставится палец.
    pub ruler_center: Point,
    /// Ход пальца вдоль линейки на полную шкалу `−200…+200`, pt (длина «ход 400 pt»).
    pub full_travel_pt: f64,
}

/// Шкалы каналов на поверхности по раскладке раздела и числам вида. Единственное место, где рамки колонки
/// превращаются в положение на поверхности: виджет раздела строится по тем же `ColumnFrames` / `BarsFrames`, поэтому
/// ответ не может разойтись с нарисованным. Публична, чтобы сквозные тесты приложения (без доступа к виджету) могли
/// спросить место шкалы у раскладки из `ScreenState`.
pub fn bars_ruler_placements(
    layout: &super::CalibrationLayout,
    config: &CalibrationWidgetsConfig,
) -> Result<[BarsRulerPlacement; 3], BarsFramesError> {
    let column = super::ColumnFrames::of(layout.zones().knobs, config);
    let frames = BarsFrames::of(column.content, config)?;
    Ok(frames.rows.map(|row| {
        let ruler_eye = Rect::from_xywh(
            row.min_x(),
            row.min_y() + config.row_title_height_pt,
            row.size.width,
            row.size.height - config.row_title_height_pt,
        );
        let ruler = layout.eye_rect_on_surface(ruler_eye);
        BarsRulerPlacement {
            row: layout.eye_rect_on_surface(row),
            ruler_center: ruler.center(),
            full_travel_pt: config.knob_full_travel_pt,
        }
    }))
}

#[cfg(test)]
#[path = "bars_frames_tests.rs"]
mod tests;
