//! Окно диаграммы CIE и перевод цветности в координаты картинки (решения 332–334).
//!
//! Диаграмма — квадрат: одна и та же длина по обеим осям, иначе картинка растянет спектральный локус. Окно задаёт,
//! какой кусок плоскости цветностей виден: левая граница, нижняя граница и сторона. Окно СВОЁ у каждой шкалы (xy и
//! u′v′ занимают разные доли плоскости) и лежит в настройках, а не в коде. Ось «вверх» диаграммы — строки картинки
//! ВНИЗ: строка 0 — верх окна.

use isp_model::color::chromaticity::Chromaticity;
use scope_contract::CieDiagram;
use serde::{Deserialize, Serialize};

/// Окно диаграммы: квадрат в координатах шкалы (x и y либо u′ и v′).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartWindow {
    /// Левая граница (значение по горизонтальной оси).
    pub left: f32,
    /// Нижняя граница (значение по вертикальной оси).
    pub bottom: f32,
    /// Сторона квадрата в единицах шкалы.
    pub extent: f32,
}

impl ChartWindow {
    /// Окно xy 1931: вмещает весь спектральный локус (x 0…0,735, y 0…0,834) с запасом не меньше 0,015 со всех сторон
    /// (проверяет тест). `[estimate]`.
    pub const XY_DEFAULT: Self = Self {
        left: -0.04,
        bottom: -0.03,
        extent: 0.88,
    };

    /// Окно u′v′ 1976: вмещает весь локус (u′ 0…0,623, v′ 0…0,587) с запасом не меньше 0,03. `[estimate]`.
    pub const UV_DEFAULT: Self = Self {
        left: -0.03,
        bottom: -0.03,
        extent: 0.68,
    };

    /// Верхняя граница окна. ЕДИНСТВЕННОЕ место, где она считается: правило и сетка берут одно и то же f32.
    pub fn top(self) -> f32 {
        self.bottom + self.extent
    }
}

/// Цветность на осях диаграммы `diagram`: `[x, y]` либо `[u′, v′]`.
pub fn chart_point(diagram: CieDiagram, chromaticity: Chromaticity) -> [f64; 2] {
    match diagram {
        CieDiagram::Xy1931 => [chromaticity.x, chromaticity.y],
        CieDiagram::UvPrime1976 => {
            let denominator = -2.0 * chromaticity.x + 12.0 * chromaticity.y + 3.0;
            [4.0 * chromaticity.x / denominator, 9.0 * chromaticity.y / denominator]
        }
    }
}

/// Обратный перевод: координаты шкалы → `[x, y]` (нужен цветам по месту). `None` — знаменатель не положителен
/// (точка вне области, где u′v′ определены).
pub fn xy_from_chart(diagram: CieDiagram, point: [f32; 2]) -> Option<[f32; 2]> {
    match diagram {
        CieDiagram::Xy1931 => Some(point),
        CieDiagram::UvPrime1976 => {
            let denominator = 6.0 * point[0] - 16.0 * point[1] + 12.0;
            (denominator > 0.0).then(|| [9.0 * point[0] / denominator, 4.0 * point[1] / denominator])
        }
    }
}

/// Куда ложится точка шкалы на картинке: окно + шкала.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartProjection {
    diagram: CieDiagram,
    window: ChartWindow,
}

impl ChartProjection {
    /// Проекция шкалы `diagram` с окном `window`.
    pub fn new(diagram: CieDiagram, window: ChartWindow) -> Self {
        Self { diagram, window }
    }

    /// Окно проекции.
    pub fn window(&self) -> ChartWindow {
        self.window
    }

    /// Шкала проекции.
    pub fn diagram(&self) -> CieDiagram {
        self.diagram
    }

    /// Точка на картинке в долях её размера (`x` вправо, `y` ВНИЗ) для цветности `chromaticity`; может выйти за
    /// `0..=1`, если цветность вне окна.
    pub fn picture_point(&self, chromaticity: Chromaticity) -> [f32; 2] {
        let point = chart_point(self.diagram, chromaticity);
        let extent = f64::from(self.window.extent);
        [
            ((point[0] - f64::from(self.window.left)) / extent) as f32,
            ((f64::from(self.window.top()) - point[1]) / extent) as f32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d65_in_uv_matches_the_published_values() {
        let [u, v] = chart_point(CieDiagram::UvPrime1976, Chromaticity::D65);
        assert!((u - 0.1978).abs() < 5e-4 && (v - 0.4683).abs() < 5e-4, "{u} {v}");
    }

    #[test]
    fn chart_and_back_round_trips_in_both_diagrams() {
        for diagram in [CieDiagram::Xy1931, CieDiagram::UvPrime1976] {
            let chart = chart_point(diagram, Chromaticity { x: 0.265, y: 0.690 });
            let back = xy_from_chart(diagram, [chart[0] as f32, chart[1] as f32]).unwrap();
            assert!(
                (back[0] - 0.265).abs() < 1e-5 && (back[1] - 0.690).abs() < 1e-5,
                "{diagram:?}"
            );
        }
        assert_eq!(xy_from_chart(CieDiagram::UvPrime1976, [0.0, 1.0]), None);
    }

    #[test]
    fn the_picture_point_puts_the_window_corners_at_the_picture_corners_with_y_down() {
        let window = ChartWindow::XY_DEFAULT;
        let projection = ChartProjection::new(CieDiagram::Xy1931, window);
        let top_left = projection.picture_point(Chromaticity {
            x: f64::from(window.left),
            y: f64::from(window.top()),
        });
        assert!(top_left[0].abs() < 1e-6 && top_left[1].abs() < 1e-6, "{top_left:?}");
        let bottom_right = projection.picture_point(Chromaticity {
            x: f64::from(window.left + window.extent),
            y: f64::from(window.bottom),
        });
        assert!((bottom_right[0] - 1.0).abs() < 1e-6 && (bottom_right[1] - 1.0).abs() < 1e-6);
    }
}
