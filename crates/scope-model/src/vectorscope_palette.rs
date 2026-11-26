//! Таблицы цвета «по месту» для вектороскопа: по осям прибора строят [`PositionPalette`] (решение 267).
//!
//! Вектороскоп красится ЦВЕТОМ ТОГО МЕСТА, куда лёг отсчёт: направление от центра — оттенок, расстояние —
//! насыщенность, светлота у всех цветов одна ([`crate::RenderSettings::vectorscope_lightness`]). Таблица строится
//! ПО ОСЯМ: у Y'CbCr и OKLab своя геометрия цвета, но формула показа одна ([`PositionPalette::colour_at`]).
//!
//! # Как строится запись угла
//! Из центра идёт луч под углом `θ`. Цвет вдоль луча растёт от нейтрали (серое светлоты `L`) по хроме оси, пока
//! все каналы экрана укладываются в `0..1`; дальше луч выходит из охвата экрана, и цвет остаётся тем, что был на
//! пределе (оттенок сохраняется, хрома урезается ровно настолько, чтобы цвет можно было показать). Запись угла —
//! цвет на пределе и радиус предела `r_lim` в клетках, не дальше края сетки. Показ между центром и пределом
//! линейно смешивает нейтраль и край — у Y'CbCr это ТОЧНАЯ формула (цветоразность линейна по радиусу), у OKLab —
//! сознательное упрощение: настоящая кривая OKLab по радиусу не линейна в кодировке экрана, но отличие невелико, а
//! шейдеру достаточно одного `mix`.
//!
//! * Y'CbCr BT.709: `Cb = dx / (2·half·gain)`, `Cr = dy / (2·half·gain)`, `ΔR = 1,5748·Cr`, `ΔB = 1,8556·Cb`,
//!   `ΔG = −(0,2126·ΔR + 0,0722·ΔB) / 0,7152`; цвет = `L + r·Δ` (кодировка сигнала, то есть R'G'B').
//! * OKLab: `a = dx / scale`, `b = dy / scale`, `scale = half / range`; цвет = OKLab `(L, a, b)` → линейный свет
//!   экрана (матрицы правила, для P3 — в P3) → кодировка sRGB (передача экрана). Предел ищется шагом в одну клетку
//!   до первого выхода из охвата и бисекцией внутри последнего шага.

use crate::oklab_rule::OklabVectorscopeRule;
use crate::position_palette::{POSITION_PALETTE_ANGLES, PositionPalette, PositionPaletteEntry};
use crate::trace_rules::BT709_LUMA_WEIGHTS;
use crate::vectorscope_geometry::VectorscopeGeometry;
use crate::vectorscope_ycbcr::YCbCrVectorscopeRule;

/// Сколько шагов бисекции предела охвата (точность далеко за пределами f32 в клетках сетки).
const GAMUT_BISECTION_STEPS: u32 = 32;

/// Шаг поиска первого выхода луча из охвата, клеток.
const GAMUT_SEARCH_STEP_CELLS: f64 = 1.0;

/// Допуск каналов экрана на границе охвата (ошибка матриц f32).
const GAMUT_TOLERANCE: f32 = 1.0e-6;

/// Луч из центра сетки.
struct Ray {
    /// Единичное направление: `x` вправо, `y` ВВЕРХ (как ось цвета, не как строки картинки).
    direction: [f64; 2],
    /// Расстояние от центра до края сетки вдоль луча, клеток.
    grid_edge_cells: f64,
}

/// Цвет на пределе луча и радиус предела.
struct RayEnd {
    colour: [f32; 3],
    radius_cells: f64,
}

/// Таблица по лучам: `end_of_ray` отдаёт предел каждого из `POSITION_PALETTE_ANGLES` равноотстоящих лучей.
fn palette_from_rays(
    geometry: &VectorscopeGeometry,
    neutral: [f32; 3],
    end_of_ray: impl Fn(&Ray) -> RayEnd,
) -> PositionPalette {
    let half_extent = geometry.half_extent_cells();
    let entries = std::array::from_fn(|index| {
        let angle = std::f64::consts::TAU * index as f64 / POSITION_PALETTE_ANGLES as f64;
        let direction = [angle.cos(), angle.sin()];
        // Край сетки вдоль луча: квадрат со стороной «полный размах» (`half_extent` в каждую сторону).
        let grid_edge_cells = half_extent / direction[0].abs().max(direction[1].abs());
        let end = end_of_ray(&Ray {
            direction,
            grid_edge_cells,
        });
        PositionPaletteEntry {
            edge_colour: end.colour,
            edge_radius_cells: end.radius_cells as f32,
        }
    });
    PositionPalette::new(entries, neutral, geometry.centre_offset_cells() as f32)
}

impl YCbCrVectorscopeRule {
    /// Таблица цвета по месту для светлоты `lightness` (серое R'G'B' = светлота). Центр сетки — нейтраль.
    pub fn position_palette(&self, lightness: f32) -> PositionPalette {
        let geometry = self.geometry();
        // Клеток на единицу Cb/Cr: та же величина, что у проекций правила (`2 · half · gain`).
        let cells_per_unit = 2.0 * geometry.half_extent_cells() * f64::from(self.gain());
        let base = f64::from(lightness);
        let [luma_red, luma_green, luma_blue] = BT709_LUMA_WEIGHTS;
        palette_from_rays(&geometry, [lightness; 3], |ray| {
            // Цветоразности и смещения каналов на ОДНУ клетку вдоль луча.
            let cb = ray.direction[0] / cells_per_unit;
            let cr = ray.direction[1] / cells_per_unit;
            let delta_red = 2.0 * (1.0 - luma_red) * cr;
            let delta_blue = 2.0 * (1.0 - luma_blue) * cb;
            let delta_green = -(luma_red * delta_red + luma_blue * delta_blue) / luma_green;
            let deltas = [delta_red, delta_green, delta_blue];
            // Предел охвата: наибольший радиус, при котором каждый канал остаётся в 0..=1.
            let gamut_radius = deltas
                .iter()
                .filter_map(|&delta| {
                    if delta > 1.0e-12 {
                        Some((1.0 - base) / delta)
                    } else if delta < -1.0e-12 {
                        Some(base / -delta)
                    } else {
                        None
                    }
                })
                .fold(f64::INFINITY, f64::min);
            let radius_cells = gamut_radius.min(ray.grid_edge_cells);
            RayEnd {
                colour: deltas.map(|delta| (base + radius_cells * delta).clamp(0.0, 1.0) as f32),
                radius_cells,
            }
        })
    }
}

impl OklabVectorscopeRule {
    /// Таблица цвета по месту для светлоты `lightness` (OKLab `L`). Центр сетки — нейтраль той же светлоты.
    pub fn position_palette(&self, lightness: f32) -> PositionPalette {
        let geometry = self.geometry();
        let cells_per_chroma = f64::from(self.scale());
        let linear_at = |ray: &Ray, radius_cells: f64| -> [f32; 3] {
            let a = ray.direction[0] * radius_cells / cells_per_chroma;
            let b = ray.direction[1] * radius_cells / cells_per_chroma;
            self.display_linear_from_lab([lightness, a as f32, b as f32])
        };
        let in_gamut = |linear: [f32; 3]| {
            linear
                .iter()
                .all(|channel| (-GAMUT_TOLERANCE..=1.0 + GAMUT_TOLERANCE).contains(channel))
        };
        let neutral = self
            .display_linear_from_lab([lightness, 0.0, 0.0])
            .map(encode_srgb_channel);
        palette_from_rays(&geometry, neutral, |ray| {
            // Первый шаг, где луч вышел из охвата, и последний шаг внутри; нет выхода — предел у края сетки.
            let mut inside = 0.0_f64;
            let mut outside = None;
            let mut radius = GAMUT_SEARCH_STEP_CELLS;
            while radius < ray.grid_edge_cells {
                if in_gamut(linear_at(ray, radius)) {
                    inside = radius;
                    radius += GAMUT_SEARCH_STEP_CELLS;
                } else {
                    outside = Some(radius);
                    break;
                }
            }
            let outside = outside.unwrap_or(ray.grid_edge_cells);
            let radius_cells = if in_gamut(linear_at(ray, outside)) {
                outside
            } else {
                bisect_gamut_edge(inside, outside, |radius| in_gamut(linear_at(ray, radius)))
            };
            RayEnd {
                colour: linear_at(ray, radius_cells).map(encode_srgb_channel),
                radius_cells,
            }
        })
    }
}

/// Бисекция: `inside` в охвате, `outside` нет; возвращает наибольший найденный радиус в охвате.
fn bisect_gamut_edge(mut inside: f64, mut outside: f64, in_gamut: impl Fn(f64) -> bool) -> f64 {
    for _ in 0..GAMUT_BISECTION_STEPS {
        let middle = (inside + outside) / 2.0;
        if in_gamut(middle) {
            inside = middle;
        } else {
            outside = middle;
        }
    }
    inside
}

/// Линейный свет экрана → кодировка экрана (передача sRGB, одна и для sRGB, и для Display P3), канал в `0..=1`.
fn encode_srgb_channel(linear: f32) -> f32 {
    let clamped = f64::from(linear).clamp(0.0, 1.0);
    let encoded = if clamped <= 0.003_130_8 {
        12.92 * clamped
    } else {
        1.055 * clamped.powf(1.0 / 2.4) - 0.055
    };
    encoded as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_encoding_hits_the_known_points_and_clamps() {
        assert_eq!(encode_srgb_channel(0.0), 0.0);
        assert!((encode_srgb_channel(1.0) - 1.0).abs() < 1e-6);
        // Линейные 0,2140 — это 50 % по кодировке sRGB.
        assert!((encode_srgb_channel(0.2140) - 0.5).abs() < 1e-3);
        assert_eq!(encode_srgb_channel(-0.5), 0.0);
        assert!((encode_srgb_channel(7.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bisection_finds_the_last_point_inside_a_known_boundary() {
        let found = bisect_gamut_edge(0.0, 10.0, |radius| radius <= 3.25);
        assert!((found - 3.25).abs() < 1e-6, "{found}");
    }

    #[test]
    fn ycbcr_entries_follow_the_closed_form_of_the_chroma_limit() {
        let rule = YCbCrVectorscopeRule::new(256, 1.0).unwrap();
        let palette = rule.position_palette(0.7);
        // Угол 90° (Cr > 0, красный): Cb = 0, ΔR на клетку = 1,5748 / (2 · 127), предел по R: (1 − 0,7) / ΔR.
        let entry = palette.entries()[POSITION_PALETTE_ANGLES / 4];
        let delta_red_per_cell = 1.5748 / (2.0 * 127.0);
        let expected_limit = 0.3 / delta_red_per_cell;
        assert!(
            (f64::from(entry.edge_radius_cells) - expected_limit).abs() < 1e-3,
            "{} против {expected_limit}",
            entry.edge_radius_cells
        );
        // На пределе красный канал ровно 1, зелёный убывает (ΔG < 0), синий не меняется (Cb = 0).
        assert!((entry.edge_colour[0] - 1.0).abs() < 1e-5, "{:?}", entry.edge_colour);
        assert!(entry.edge_colour[1] < 0.7, "{:?}", entry.edge_colour);
        assert!((entry.edge_colour[2] - 0.7).abs() < 1e-5, "{:?}", entry.edge_colour);
        assert_eq!(palette.neutral(), [0.7; 3]);
        assert_eq!(palette.centre_cells(), 128.5);
    }
}
