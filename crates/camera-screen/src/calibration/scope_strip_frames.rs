//! Рамки частей полоски прибора (решения 258, 290, 291, 321, 325): имя слева, точки страниц по центру (или левее,
//! если по центру им тесно рядом с кнопкой «2×»), кнопка «2×» и переключатель осей
//! вектороскопа и капсула белого справа. Чистая функция рамки полоски и чисел виджетов — ей же пользуются виджет
//! полоски и тесты соседства, поэтому «что где стоит» считается в одном месте (как `ColumnFrames` колонки ручек).
//!
//! Все рамки — на всю высоту полоски (44 pt, решение 291): это и есть зона касания цели, соседние по ширине не
//! перекрываются. Рисунок внутри (точка 6 pt, капсула сегмента 28 pt) меньше своей зоны.

use scope_contract::ScopeKind;
use ui_core::geometry::Rect;

use super::widgets_config::CalibrationWidgetsConfig;

/// Рамки частей полоски в системе, в которой задана сама полоска.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeStripFrames {
    /// Место имени прибора слева (имя прижато к левому краю места).
    pub title: Rect,
    /// Ячейки точек страниц в порядке `ScopeKind::ALL`, встык друг к другу по центру полоски.
    pub page_dots: [Rect; ScopeKind::ALL.len()],
    /// Переключатель осей вектороскопа (виден только на странице вектороскопа), левее места белого.
    pub axes_toggle: Rect,
    /// Кнопка «2×» вектороскопа (решение 325), левее переключателя осей; видна только на странице вектороскопа.
    pub zoom_toggle: Rect,
    /// Место капсулы белого справа (`WhiteCapsule`): на каждой странице, оси стоят левее неё (решение 290).
    pub trailing: Rect,
}

impl ScopeStripFrames {
    /// Раскладывает части по полоске `strip`.
    pub fn of(strip: Rect, config: &CalibrationWidgetsConfig) -> Self {
        let band = |left: f64, width: f64| Rect::from_xywh(left, strip.min_y(), width, strip.size.height);
        let inset = config.scope_label_inset_pt;
        let gap = config.scope_strip_gap_pt;

        let trailing_width = config.scope_strip_trailing_width_pt;
        let trailing = band(strip.max_x() - inset - trailing_width, trailing_width);

        let toggle_width = 2.0 * config.scope_axes_segment_width_pt + config.segment_gap_pt;
        let axes_toggle = band(trailing.min_x() - gap - toggle_width, toggle_width);
        let zoom_width = config.scope_zoom_segment_width_pt;
        let zoom_toggle = band(axes_toggle.min_x() - gap - zoom_width, zoom_width);

        let cell_width = config.scope_page_dot_cell_width_pt;
        let dots_width = cell_width * ScopeKind::ALL.len() as f64;
        // Точки — по центру полоски; если там они налезли бы на переключатель осей (четыре страницы на узком экране,
        // решение 321), ряд сдвигается влево и встаёт вплотную к переключателю через зазор. Положение одно на всех
        // страницах: место переключателя держится и там, где он скрыт, поэтому точки не прыгают при свайпе.
        let centred_left = strip.center().x - dots_width / 2.0;
        let dots_left = centred_left.min(zoom_toggle.min_x() - gap - dots_width);
        let page_dots = std::array::from_fn(|index| band(dots_left + cell_width * index as f64, cell_width));

        let title_left = strip.min_x() + inset;
        let title = band(title_left, (page_dots[0].min_x() - gap - title_left).max(0.0));
        Self {
            title,
            page_dots,
            axes_toggle,
            zoom_toggle,
            trailing,
        }
    }
}

#[cfg(test)]
mod tests {
    use ui_kit::tokens::{TextRoleKind, UiKitTokens};
    use ui_kit::view::{LabelPainter, PxPerPt};

    use super::*;
    use crate::calibration::CalibrationConfig;
    use crate::calibration::layout_tests::{ALL_ORIENTATIONS, calibration_layout, screen};
    use crate::mode_behaviour::LayoutPolicy;
    use shell_contract::orientation::DeviceOrientation;

    fn overlaps(first: Rect, second: Rect) -> bool {
        first.min_x() < second.max_x() - 1e-9
            && second.min_x() < first.max_x() - 1e-9
            && first.min_y() < second.max_y() - 1e-9
            && second.min_y() < first.max_y() - 1e-9
    }

    fn parts(frames: &ScopeStripFrames) -> Vec<(&'static str, Rect)> {
        let mut all = vec![("имя", frames.title)];
        all.extend(frames.page_dots.iter().map(|cell| ("точка", *cell)));
        all.push(("оси", frames.axes_toggle));
        all.push(("зум", frames.zoom_toggle));
        all.push(("белое", frames.trailing));
        all
    }

    #[test]
    fn title_dots_axes_and_the_white_slot_never_share_a_point_on_the_tightest_page_in_every_orientation() {
        let config = CalibrationConfig::default();
        for orientation in ALL_ORIENTATIONS {
            let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
            let strip = layout.scope_strip();
            let frames = ScopeStripFrames::of(strip, &config.widgets);
            let all = parts(&frames);
            for (index, (first_name, first)) in all.iter().enumerate() {
                assert!(
                    first.min_x() >= strip.min_x() - 1e-9 && first.max_x() <= strip.max_x() + 1e-9,
                    "{orientation:?}: {first_name} вылезает из полоски: {first:?}"
                );
                assert!(
                    (first.size.height - strip.size.height).abs() < 1e-9,
                    "{orientation:?}: {first_name}: цель на всю высоту полоски"
                );
                for (second_name, second) in &all[index + 1..] {
                    assert!(
                        !overlaps(*first, *second),
                        "{orientation:?}: {first_name} {first:?} перекрывает {second_name} {second:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_title_slot_holds_the_longest_scope_name_and_the_dots_stay_centred_or_beside_the_axes() {
        let tokens = UiKitTokens::default();
        let ppp = screen().point_scale().pixels_per_point();
        let painter = LabelPainter::new(&tokens.typography, PxPerPt::new(ppp as f32).expect("масштаб"));
        let config = CalibrationConfig::default();
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
        let strip = layout.scope_strip();
        let frames = ScopeStripFrames::of(strip, &config.widgets);
        for title in ["RGB WAVEFORM", "LUMA WAVEFORM", "RGB PARADE", "VECTORSCOPE"] {
            let run = painter.layout(title, TextRoleKind::Caption).expect("раскладка подписи");
            let width_pt = f64::from(run.advance_px) / ppp;
            assert!(
                width_pt <= frames.title.size.width,
                "{title}: {width_pt} pt в месте {}",
                frames.title.size.width
            );
        }
        // Точки — по центру полоски, если там им хватает места; иначе вплотную к переключателю осей через зазор
        // (четыре страницы на узком экране, решение 321). В любом случае точки не левее имени и не на осях.
        let dots_left = frames.page_dots[0].min_x();
        let dots_right = frames.page_dots[frames.page_dots.len() - 1].max_x();
        let centred = ((dots_left + dots_right) / 2.0 - strip.center().x).abs() < 1e-9;
        let against_zoom = (frames.zoom_toggle.min_x() - config.widgets.scope_strip_gap_pt - dots_right).abs() < 1e-9;
        assert!(centred || against_zoom, "точки по центру или вплотную к кнопке «2×»");
        assert!(dots_right <= frames.zoom_toggle.min_x() - config.widgets.scope_strip_gap_pt + 1e-9);
        assert!(
            frames.zoom_toggle.max_x() < frames.axes_toggle.min_x(),
            "«2×» левее осей"
        );
        assert!(dots_left >= frames.title.max_x());
        // Место белого — справа на каждой странице, оси стоят левее него.
        assert!(frames.axes_toggle.max_x() < frames.trailing.min_x());
        assert!((strip.max_x() - frames.trailing.max_x() - config.widgets.scope_label_inset_pt).abs() < 1e-9);
    }
}
