//! Цвет пикселя гистограммы в стиле Resolve (решение 337): тёмная полупрозрачная заливка + яркая тонкая линия по верху.
//! Эталон ветки `SCOPE_RENDER_HISTOGRAM` шейдера (`histogram_edge_coverage`, `HistogramBlend`): тот же порядок f32.
//!
//! # Линия верха
//! Линия — расстояние от центра пикселя до ЛОМАНОЙ через верхушки столбцов `(x + 0,5, высота)`. Считается по двум
//! отрезкам, соседним с пикселем (левый: столбец `x − 1 → x`, правый: `x → x + 1`): так линия идёт и по крутому боку
//! (вертикальный склон — длинный отрезок ломаной, пиксели рядом с ним видят его как соседний), а не только горизонтальной
//! «шапкой» над столбцом. Любой другой отрезок ломаной дальше пикселя по горизонтали не меньше чем на ширину столбца,
//! поэтому при толщине до [`crate::render_settings::MAX_HISTOGRAM_EDGE_WIDTH_PX`] два соседних отрезка дают то же
//! расстояние, что вся ломаная, с точностью до краёв сглаживающей полутени. Стоимость — три высоты столбца на пиксель.
//! Отрезок, у которого обе высоты нулевые, не рисуется: пустой график не тянет линию вдоль всей базовой линии.
//! Покрытие `clamp(толщина / 2 + 0,5 − расстояние, 0, 1)`: тот же закон, что у линий сетки (полупиксельная полутень).
//!
//! # Сложение каналов
//! Каналы (три у наложения RGB, один у полосы PARADE и Y) складываются как свет: цвета заливок суммируются, прозрачность
//! заливки — по самому высокому покрытию (не по сумме, чтобы три канала не превращались в сплошную массу); линии рисуются
//! поверх своими цветами и тоже суммируются (где совпали все три — белый), прозрачность линии — по самой яркой.

use crate::histogram_strips::HistogramLook;
use crate::trace_painter::TracePixel;

/// Точка ломаной в координатах столбца: `x` — смещение от центра столбца пикселя в пикселях, `y` — высота над базой.
pub(crate) type CurvePoint = [f32; 2];

/// Покрытие пикселя линией верха. `point` — центр пикселя (`[0, расстояние до базовой линии]`), `left`, `centre`, `right`
/// — верхушки столбцов `x − 1`, `x`, `x + 1` (`[-1, h], [0, h], [1, h]`); `edge_width_px == 0` — линии нет.
pub(crate) fn edge_coverage(point: CurvePoint, heights: [f32; 3], edge_width_px: f32) -> f32 {
    if edge_width_px <= 0.0 {
        return 0.0;
    }
    let [left, centre, right] = heights;
    let mut nearest = f32::INFINITY;
    for (start, end) in [([-1.0, left], [0.0, centre]), ([0.0, centre], [1.0, right])] {
        // Отрезок над пустыми столбцами — не часть графика.
        if start[1].max(end[1]) > 0.0 {
            nearest = nearest.min(distance_to_segment(point, start, end));
        }
    }
    (edge_width_px * 0.5 + 0.5 - nearest).clamp(0.0, 1.0)
}

/// Расстояние от точки до отрезка. Отрезок графика всегда шириной в 1 px, поэтому длина не нулевая; защита от нуля — на
/// случай вызова с другими точками.
fn distance_to_segment(point: CurvePoint, start: CurvePoint, end: CurvePoint) -> f32 {
    let along = [end[0] - start[0], end[1] - start[1]];
    let relative = [point[0] - start[0], point[1] - start[1]];
    let length_squared = along[0] * along[0] + along[1] * along[1];
    let fraction = if length_squared > 0.0 {
        ((relative[0] * along[0] + relative[1] * along[1]) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let offset = [relative[0] - along[0] * fraction, relative[1] - along[1] * fraction];
    (offset[0] * offset[0] + offset[1] * offset[1]).sqrt()
}

/// Накопитель каналов одного пикселя гистограммы: заливки и линии отдельно.
pub(crate) struct HistogramBlend {
    fill_light: [f32; 3],
    fill_coverage: f32,
    edge_light: [f32; 3],
    edge_coverage: f32,
}

impl HistogramBlend {
    /// Пустой пиксель.
    pub(crate) fn new() -> Self {
        Self {
            fill_light: [0.0; 3],
            fill_coverage: 0.0,
            edge_light: [0.0; 3],
            edge_coverage: 0.0,
        }
    }

    /// Добавляет канал цвета `color`: `fill` — покрытие его заливкой, `edge` — покрытие его линией (оба `0..=1`).
    pub(crate) fn add_channel(&mut self, color: [f32; 3], fill: f32, edge: f32) {
        for (channel, value) in color.into_iter().enumerate() {
            self.fill_light[channel] += fill * value;
            self.edge_light[channel] += edge * value;
        }
        self.fill_coverage = self.fill_coverage.max(fill);
        self.edge_coverage = self.edge_coverage.max(edge);
    }

    /// Итоговый след: линия «над» заливкой, прямая альфа; нет ни заливки, ни линии — [`TracePixel::ABSENT`].
    pub(crate) fn finish(&self, look: &HistogramLook) -> TracePixel {
        let fill_alpha = look.fill_opacity * self.fill_coverage;
        let edge_alpha = self.edge_coverage;
        let alpha = edge_alpha + fill_alpha * (1.0 - edge_alpha);
        if alpha <= 0.0 {
            return TracePixel::ABSENT;
        }
        let color = std::array::from_fn(|channel| {
            // Свет линии не больше полной яркости (три совпавших канала дают как раз 1); заливка затемнена.
            let edge = self.edge_light[channel].min(1.0);
            let fill = look.fill_opacity * look.fill_tint * self.fill_light[channel];
            ((edge + fill * (1.0 - edge_alpha)) / alpha).clamp(0.0, 1.0)
        });
        TracePixel { color, alpha }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOK: HistogramLook = HistogramLook {
        fill_opacity: 0.4,
        fill_tint: 0.5,
        edge_width_px: 1.0,
        strip_gap: 0.0,
    };

    #[test]
    fn a_flat_curve_gives_a_line_that_fades_with_the_distance() {
        let flat = [10.0; 3];
        assert_eq!(edge_coverage([0.0, 10.0], flat, 1.0), 1.0);
        // Расстояние 0,5: 0,5 + 0,5 − 0,5 = 0,5.
        assert!((edge_coverage([0.0, 10.5], flat, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(edge_coverage([0.0, 12.0], flat, 1.0), 0.0);
        assert_eq!(edge_coverage([0.0, 10.0], flat, 0.0), 0.0, "толщина 0 — линии нет");
    }

    #[test]
    fn a_steep_flank_is_drawn_beside_the_top_not_only_over_it() {
        // Правый столбец выше на 20 px: пиксель рядом с серединой склона на высоте 15 тоже на линии.
        let step = [10.0, 10.0, 30.0];
        let beside_the_flank = edge_coverage([0.0, 15.0], step, 1.0);
        assert!(
            (beside_the_flank - 0.75).abs() < 0.01,
            "расстояние до склона 5 / √401 ≈ 0,25: {beside_the_flank}"
        );
        assert_eq!(edge_coverage([0.0, 5.0], step, 1.0), 0.0, "под склоном линии нет");
    }

    #[test]
    fn an_empty_neighbourhood_has_no_line_along_the_baseline() {
        assert_eq!(edge_coverage([0.0, 0.5], [0.0; 3], 3.0), 0.0);
        // Но у края графика линия сходит к базовой линии.
        assert!(edge_coverage([0.0, 0.5], [0.0, 0.0, 20.0], 1.0) > 0.0);
    }

    #[test]
    fn fill_stays_translucent_and_the_edge_is_opaque_and_on_top() {
        let mut fill_only = HistogramBlend::new();
        fill_only.add_channel([1.0, 0.0, 0.0], 1.0, 0.0);
        let pixel = fill_only.finish(&LOOK);
        assert_eq!(pixel.alpha, 0.4);
        assert_eq!(
            pixel.color,
            [0.5, 0.0, 0.0],
            "заливка затемнена множителем fill_tint = 0,5"
        );
        let mut with_edge = HistogramBlend::new();
        with_edge.add_channel([1.0, 0.0, 0.0], 1.0, 1.0);
        let pixel = with_edge.finish(&LOOK);
        assert_eq!((pixel.alpha, pixel.color), (1.0, [1.0, 0.0, 0.0]));
        assert_eq!(HistogramBlend::new().finish(&LOOK), TracePixel::ABSENT);
    }

    #[test]
    fn channels_add_in_colour_but_not_in_opacity() {
        let mut blend = HistogramBlend::new();
        blend.add_channel([1.0, 0.0, 0.0], 1.0, 0.0);
        blend.add_channel([0.0, 1.0, 0.0], 1.0, 0.0);
        let pixel = blend.finish(&LOOK);
        assert_eq!(pixel.alpha, 0.4, "прозрачность по самому высокому покрытию");
        assert_eq!(
            pixel.color,
            [0.5, 0.5, 0.0],
            "красный и зелёный дают жёлтый (затемнённый)"
        );
    }
}
