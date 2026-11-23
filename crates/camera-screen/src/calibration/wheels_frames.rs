//! Геометрия колонки ручек и вида колёс: ОДНА чистая функция от зоны колонки и конфига (сессия 14b2).
//!
//! Её читают и виджеты (`widget/calibration`), и фасад экрана (запрос «где круг колеса на поверхности»), и тесты:
//! рамки не считаются дважды, поэтому сквозной тест не может разойтись с тем, что рисует экран.
//!
//! Функции не знают про сцену, поворот и узлы: на входе прямоугольник в ЛЮБОЙ системе (глазные координаты
//! раскладки или система корня раздела — рамки просто сдвигаются), на выходе рамки в той же системе.

use thiserror::Error;
use ui_core::geometry::Rect;

use super::widgets_config::CalibrationWidgetsConfig;

/// Рамки колонки ручек: шапка, строка переключателя вида и окно под ними.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColumnFrames {
    /// Шапка: основа тона, «↻», «чистяк».
    pub header: Rect,
    /// Строка переключателя SCALES | WHEELS: не прокручивается, видна в обоих видах.
    pub switcher: Rect,
    /// Окно вида на всю ширину панели (содержимое уже на отступы и едет внутри окна).
    pub window: Rect,
    /// Рамка содержимого вида: внутренняя ширина колонки и высота окна.
    pub content: Rect,
}

impl ColumnFrames {
    /// Рамки внутри стеклянной панели `panel`.
    pub(crate) fn of(panel: Rect, config: &CalibrationWidgetsConfig) -> Self {
        let padding = config.panel_padding_pt;
        let inner_width = (panel.size.width - 2.0 * padding).max(0.0);
        let header = Rect::from_xywh(
            panel.min_x() + padding,
            panel.min_y() + padding,
            inner_width,
            config.header_height_pt,
        );
        let switcher = Rect::from_xywh(
            header.min_x(),
            header.max_y(),
            inner_width,
            config.view_switcher_height_pt,
        );
        let window = Rect::from_xywh(
            panel.min_x(),
            switcher.max_y(),
            panel.size.width,
            (panel.max_y() - switcher.max_y()).max(0.0),
        );
        let content = Rect::from_xywh(panel.min_x() + padding, window.min_y(), inner_width, window.size.height);
        Self {
            header,
            switcher,
            window,
            content,
        }
    }
}

/// Почему вид колёс нельзя разложить.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub(crate) enum WheelsFramesError {
    /// После вкладок, названия и шкалы кольца круг не помещается даже наименьшего диаметра.
    #[error("круг колеса не помещается: доступно {available_pt} pt, наименьший диаметр {minimum_pt} pt")]
    NoRoomForCircle { available_pt: f64, minimum_pt: f64 },
}

/// Рамки вида колёс сверху вниз: вкладки, название с «↻» колеса, круг (и «↻» толчка в его углу), шкала кольца.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WheelsFrames {
    /// Ряд вкладок LIFT · GAMMA · GAIN · OFFSET.
    pub tabs: Rect,
    /// Строка названия выбранного колеса.
    pub title: Rect,
    /// «↻» колеса целиком: справа в строке названия.
    pub whole_reset: Rect,
    /// Квадрат круга: сторона = внешний диаметр цветного кольца оттенков.
    pub circle: Rect,
    /// «↻» толчка: правый нижний угол квадрата круга (снаружи самого круга).
    pub push_reset: Rect,
    /// Строка шкалы кольца яркости (название, значение, «↻» кольца и линейка).
    pub ring_row: Rect,
}

impl WheelsFrames {
    /// Раскладывает `content` (внутренняя ширина колонки × высота окна). Диаметр круга — по месту: вписывается
    /// в ширину и оставшуюся высоту, но не больше `wheel_circle_max_diameter_pt`, и ОКРУГЛЁН ВНИЗ до целых
    /// пикселей (`pixels_per_point`): тогда картинка кольца ложится 1:1, без полупикселя фильтрации.
    pub(crate) fn of(
        content: Rect,
        config: &CalibrationWidgetsConfig,
        pixels_per_point: f64,
    ) -> Result<Self, WheelsFramesError> {
        let gap = config.wheel_block_gap_pt;
        let ring_row_height = config.row_title_height_pt + config.wheel_ring_ruler_height_pt;
        // Четыре блока и три промежутка между ними; снизу — отступ панели, как у списка шкал.
        let fixed_height = config.wheel_tabs_height_pt
            + config.wheel_title_height_pt
            + ring_row_height
            + 3.0 * gap
            + config.panel_padding_pt;
        let available = content.size.height - fixed_height;
        let fitted = available
            .min(content.size.width)
            .min(config.wheel_circle_max_diameter_pt);
        let diameter = (fitted * pixels_per_point).floor() / pixels_per_point;
        if diameter < config.wheel_circle_min_diameter_pt {
            return Err(WheelsFramesError::NoRoomForCircle {
                available_pt: fitted,
                minimum_pt: config.wheel_circle_min_diameter_pt,
            });
        }
        let tabs = Rect::from_xywh(
            content.min_x(),
            content.min_y(),
            content.size.width,
            config.wheel_tabs_height_pt,
        );
        let title = Rect::from_xywh(
            content.min_x(),
            tabs.max_y() + gap,
            content.size.width,
            config.wheel_title_height_pt,
        );
        let reset_side = config.reset_button_diameter_pt;
        let whole_reset = Rect::from_xywh(
            title.max_x() - reset_side,
            title.min_y() + (title.size.height - reset_side) / 2.0,
            reset_side,
            reset_side,
        );
        let circle = Rect::from_xywh(
            content.min_x() + (content.size.width - diameter) / 2.0,
            title.max_y() + gap,
            diameter,
            diameter,
        );
        let push_reset = Rect::from_xywh(
            circle.max_x() - reset_side,
            circle.max_y() - reset_side,
            reset_side,
            reset_side,
        );
        let ring_row = Rect::from_xywh(
            content.min_x(),
            circle.max_y() + gap,
            content.size.width,
            ring_row_height,
        );
        Ok(Self {
            tabs,
            title,
            whole_reset,
            circle,
            push_reset,
            ring_row,
        })
    }

    /// Радиус области толчка, pt: от центра круга до внутреннего края цветного кольца (точка на краю = сила 1).
    pub(crate) fn pad_radius_pt(&self, config: &CalibrationWidgetsConfig) -> f64 {
        self.circle.size.width / 2.0 - config.wheel_ring_thickness_pt
    }
}

/// Где круг колеса стоит на поверхности (pt) — ответ на запрос фасада для сквозных тестов и инструментов.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelPadPlacement {
    /// Квадрат круга на поверхности (оси экрана, начало в левом верхнем углу): центр — центр круга, сторона —
    /// внешний диаметр цветного кольца. В альбоме квадрат остаётся квадратом, повёрнутым на четверть оборота.
    pub circle: Rect,
    /// Радиус области толчка, pt: от центра до внутреннего края кольца оттенков. Точка на краю = сила 1.
    pub pad_radius_pt: f64,
}

/// Круг колеса на поверхности по зоне колонки (глазные координаты) и раскладке. Единственное место, где рамки
/// колонки превращаются в положение на поверхности: виджет раздела строится по тем же `ColumnFrames` /
/// `WheelsFrames`, поэтому ответ не может разойтись с нарисованным.
pub(crate) fn wheel_pad_placement(
    layout: &super::CalibrationLayout,
    config: &CalibrationWidgetsConfig,
    pixels_per_point: f64,
) -> Result<WheelPadPlacement, WheelsFramesError> {
    let column = ColumnFrames::of(layout.zones().knobs, config);
    let frames = WheelsFrames::of(column.content, config, pixels_per_point)?;
    Ok(WheelPadPlacement {
        circle: layout.eye_rect_on_surface(frames.circle),
        pad_radius_pt: frames.pad_radius_pt(config),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Колонка `width` × `height` pt при 3 px на pt (как телефон в тестовом стенде).
    fn frames_for(width: f64, height: f64) -> Result<WheelsFrames, WheelsFramesError> {
        let config = CalibrationWidgetsConfig::default();
        let panel = Rect::from_xywh(0.0, 0.0, width, height);
        WheelsFrames::of(ColumnFrames::of(panel, &config).content, &config, 3.0)
    }

    #[test]
    fn portrait_column_gets_the_full_circle_and_everything_stacks_without_overlap() {
        let frames = frames_for(196.0, 666.0).expect("портретная колонка вмещает круг");
        let config = CalibrationWidgetsConfig::default();
        assert_eq!(frames.circle.size.width, 170.0, "в портрете круг упирается в максимум");
        assert!(frames.tabs.max_y() <= frames.title.min_y());
        assert!(frames.title.max_y() <= frames.circle.min_y());
        assert!(frames.circle.max_y() <= frames.ring_row.min_y());
        assert!(
            frames.ring_row.max_y() <= 666.0 - config.panel_padding_pt,
            "без прокрутки"
        );
        // «↻» толчка сидит в правом нижнем углу квадрата круга.
        assert_eq!(frames.push_reset.max_x(), frames.circle.max_x());
        assert_eq!(frames.push_reset.max_y(), frames.circle.max_y());
    }

    #[test]
    fn circle_diameter_is_a_whole_number_of_pixels_and_never_wider_than_the_column() {
        let frames = frames_for(150.0, 500.0).expect("узкая колонка");
        let content_width = 150.0 - 2.0 * CalibrationWidgetsConfig::default().panel_padding_pt;
        assert!(frames.circle.size.width <= content_width);
        let pixels = frames.circle.size.width * 3.0;
        assert!((pixels - pixels.round()).abs() < 1e-9, "{pixels}");
    }

    #[test]
    fn column_too_short_for_the_smallest_circle_is_an_error_not_a_tiny_circle() {
        let result = frames_for(196.0, 250.0);
        assert!(
            matches!(result, Err(WheelsFramesError::NoRoomForCircle { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn short_landscape_column_shrinks_the_circle_to_the_remaining_height() {
        let tall = frames_for(284.0, 329.0).expect("альбомная колонка");
        assert!(tall.circle.size.width < 170.0, "{}", tall.circle.size.width);
        assert!(tall.circle.size.width >= 48.0);
    }
}
