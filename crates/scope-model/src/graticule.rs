//! Сетка прибора как ДАННЫЕ: короткий список примитивов в нормированных координатах картинки
//! (`[0,1]²`, y вниз). GPU загружает тот же список и рисует теми же формулами покрытия; точный вид
//! подбирается в 14b.

/// Наибольшее число примитивов сетки (размер буфера на GPU). Поднято с 48 до 128 в сессии 14s ради диаграммы CIE: один
/// спектральный локус — 65 отрезков, к ним до трёх контуров охватов (9), кружок белой и кривая чёрного тела (до 47):
/// худший допустимый набор настроек — 122 (проверяет тест). Остальные приборы число примитивов не меняют.
pub const MAX_GRATICULE_PRIMITIVES: usize = 128;

/// Примитив сетки. `half_width_px` — полуширина линии в пикселях картинки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraticulePrimitive {
    /// Отрезок.
    Segment {
        /// Начало (нормированные координаты).
        from: [f32; 2],
        /// Конец.
        to: [f32; 2],
        /// Полуширина линии, пикселей.
        half_width_px: f32,
    },
    /// Кольцо (эллипс, если картинка не квадратная: вектороскоп растягивается вместе со следом).
    Ring {
        /// Центр.
        centre: [f32; 2],
        /// Радиусы по осям (нормированные).
        radius: [f32; 2],
        /// Полуширина линии, пикселей.
        half_width_px: f32,
    },
    /// Контур прямоугольника (цель цвета).
    BoxOutline {
        /// Центр.
        centre: [f32; 2],
        /// Половины сторон (нормированные).
        half_size: [f32; 2],
        /// Полуширина линии, пикселей.
        half_width_px: f32,
    },
}

impl GraticulePrimitive {
    /// Покрытие пикселя примитивом `clamp(half_width + 0,5 − distance, 0, 1)`; `point_px` — центр пикселя,
    /// `picture_px` — размер картинки в пикселях.
    pub fn coverage(&self, point_px: [f32; 2], picture_px: [f32; 2]) -> f32 {
        let (distance, half_width_px) = match *self {
            Self::Segment {
                from,
                to,
                half_width_px,
            } => {
                let start = scale_to_pixels(from, picture_px);
                let end = scale_to_pixels(to, picture_px);
                (distance_to_segment(point_px, start, end), half_width_px)
            }
            Self::Ring {
                centre,
                radius,
                half_width_px,
            } => {
                let centre = scale_to_pixels(centre, picture_px);
                let radius = scale_to_pixels(radius, picture_px);
                let normalized = ((point_px[0] - centre[0]) / radius[0]).hypot((point_px[1] - centre[1]) / radius[1]);
                ((normalized - 1.0).abs() * radius[0].min(radius[1]), half_width_px)
            }
            Self::BoxOutline {
                centre,
                half_size,
                half_width_px,
            } => {
                let centre = scale_to_pixels(centre, picture_px);
                let half_size = scale_to_pixels(half_size, picture_px);
                (distance_to_box_outline(point_px, centre, half_size), half_width_px)
            }
        };
        (half_width_px + 0.5 - distance).clamp(0.0, 1.0)
    }
}

fn scale_to_pixels(point: [f32; 2], picture_px: [f32; 2]) -> [f32; 2] {
    [point[0] * picture_px[0], point[1] * picture_px[1]]
}

fn distance_to_segment(point: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
    let along = [end[0] - start[0], end[1] - start[1]];
    let relative = [point[0] - start[0], point[1] - start[1]];
    let length_squared = along[0] * along[0] + along[1] * along[1];
    let fraction = if length_squared > 0.0 {
        ((relative[0] * along[0] + relative[1] * along[1]) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (relative[0] - along[0] * fraction).hypot(relative[1] - along[1] * fraction)
}

/// Расстояние до контура: модуль знакового расстояния прямоугольника.
fn distance_to_box_outline(point: [f32; 2], centre: [f32; 2], half_size: [f32; 2]) -> f32 {
    let excess = [
        (point[0] - centre[0]).abs() - half_size[0],
        (point[1] - centre[1]).abs() - half_size[1],
    ];
    let outside = excess[0].max(0.0).hypot(excess[1].max(0.0));
    let inside = excess[0].max(excess[1]).min(0.0);
    (outside + inside).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_coverage_is_full_on_the_line_and_zero_far_away() {
        let line = GraticulePrimitive::Segment {
            from: [0.0, 0.5],
            to: [1.0, 0.5],
            half_width_px: 0.75,
        };
        let picture = [100.0, 100.0];
        assert_eq!(line.coverage([50.0, 50.0], picture), 1.0);
        // Расстояние 1,0: 0,75 + 0,5 − 1,0 = 0,25.
        assert!((line.coverage([50.0, 51.0], picture) - 0.25).abs() < 1e-6);
        assert_eq!(line.coverage([50.0, 60.0], picture), 0.0);
    }

    #[test]
    fn ring_and_box_cover_their_outline_but_not_the_inside() {
        let picture = [100.0, 100.0];
        let ring = GraticulePrimitive::Ring {
            centre: [0.5, 0.5],
            radius: [0.4, 0.4],
            half_width_px: 0.5,
        };
        assert_eq!(ring.coverage([90.0, 50.0], picture), 1.0);
        assert_eq!(ring.coverage([50.0, 50.0], picture), 0.0);
        let outline = GraticulePrimitive::BoxOutline {
            centre: [0.5, 0.5],
            half_size: [0.1, 0.1],
            half_width_px: 0.5,
        };
        assert_eq!(outline.coverage([60.0, 50.0], picture), 1.0);
        assert_eq!(outline.coverage([50.0, 50.0], picture), 0.0);
        assert_eq!(outline.coverage([90.0, 90.0], picture), 0.0);
    }
}
