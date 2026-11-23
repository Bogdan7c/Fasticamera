//! Раскладка сетки прибора: из решённого правила и настроек — список [`GraticulePrimitive`].
//!
//! Положения находятся ЧЕРЕЗ ТО ЖЕ правило попадания, что и след: линия уровня — там, куда правило кладёт
//! серый код этого уровня; цель цвета — там, куда оно кладёт код цвета. Поэтому сетка не расходится со
//! следом ни при какой системе осей.

use crate::codes::MAX_DISPLAY_CODE;
use crate::graticule::{GraticulePrimitive, MAX_GRATICULE_PRIMITIVES};
use crate::render_settings::RenderSettings;
use crate::resolved::ResolvedScope;
use crate::rule::{Hit, SamplePosition, hits_of_code};

/// Уровни горизонтальных линий waveform и parade, доли диапазона кодов.
const LEVEL_LINE_FRACTIONS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Цвета целей вектороскопа: R, G, B, C, M, Y (каналы 0/1 умножаются на амплитуду).
const TARGET_COLORS: [[f32; 3]; 6] = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [1.0, 0.0, 1.0],
    [1.0, 1.0, 0.0],
];

/// Длина плеча креста центра в полуразмерах рамки цели.
const CROSS_ARM_IN_TARGET_HALVES: f32 = 2.0;

impl ResolvedScope {
    /// Сетка прибора. Число примитивов не больше [`MAX_GRATICULE_PRIMITIVES`] (проверяется тестом).
    pub fn graticule(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        let mut primitives = match self {
            Self::Waveform(_) | Self::RgbParade(_) => self.level_lines(settings),
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => self.vectorscope_marks(settings),
        };
        primitives.truncate(MAX_GRATICULE_PRIMITIVES);
        primitives
    }

    /// Горизонтальные линии уровней 0, 25, 50, 75, 100 %.
    fn level_lines(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        let rows = match self {
            Self::Waveform(rule) => rule.levels(),
            Self::RgbParade(rule) => rule.levels(),
            _ => return Vec::new(),
        };
        LEVEL_LINE_FRACTIONS
            .iter()
            .filter_map(|&fraction| {
                let code = (fraction * f32::from(MAX_DISPLAY_CODE)).round() as u16;
                let hit = self.first_cell_of(code_gray(code))?;
                let y = (hit.row as f32 + 0.5) / rows as f32;
                Some(GraticulePrimitive::Segment {
                    from: [0.0, y],
                    to: [1.0, y],
                    half_width_px: settings.graticule_half_width_px,
                })
            })
            .collect()
    }

    /// Крест центра, линия тона кожи, кольцо полного размаха и рамки целей.
    fn vectorscope_marks(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        let Some(geometry) = self.vectorscope_geometry() else {
            return Vec::new();
        };
        let size = geometry.size() as f32;
        let centre_cell = geometry.centre_cell() as f32;
        let centre = [(centre_cell + 0.5) / size, (centre_cell + 0.5) / size];
        let half_width_px = settings.graticule_half_width_px;
        let arm = settings.target_box_half_cells * CROSS_ARM_IN_TARGET_HALVES / size;
        let mut primitives = vec![
            GraticulePrimitive::Segment {
                from: [centre[0] - arm, centre[1]],
                to: [centre[0] + arm, centre[1]],
                half_width_px,
            },
            GraticulePrimitive::Segment {
                from: [centre[0], centre[1] - arm],
                to: [centre[0], centre[1] + arm],
                half_width_px,
            },
        ];
        if let Some(direction) = self.skin_direction(settings) {
            let reach = geometry.half_extent_cells() as f32 / size;
            primitives.push(GraticulePrimitive::Segment {
                from: centre,
                to: [centre[0] + direction[0] * reach, centre[1] + direction[1] * reach],
                half_width_px,
            });
        }
        let full_range_radius = self.full_range_radius_cells() / size;
        if full_range_radius <= 0.5 {
            primitives.push(GraticulePrimitive::Ring {
                centre,
                radius: [full_range_radius, full_range_radius],
                half_width_px,
            });
        }
        let amplitude_code = (settings.target_amplitude * f32::from(MAX_DISPLAY_CODE)).round() as u16;
        let half_size = settings.target_box_half_cells / size;
        for color in TARGET_COLORS {
            let code = color.map(|channel| if channel > 0.0 { amplitude_code } else { 0 });
            // Цель вне сетки (большое усиление) просто не рисуется: следа в этом месте тоже быть не может.
            if let Some(hit) = self.first_cell_of(code) {
                primitives.push(GraticulePrimitive::BoxOutline {
                    centre: [(hit.column as f32 + 0.5) / size, (hit.row as f32 + 0.5) / size],
                    half_size: [half_size, half_size],
                    half_width_px,
                });
            }
        }
        primitives
    }

    /// Первая ячейка, куда правило кладёт код (отсчёт в начале картинки ширины 1).
    fn first_cell_of(&self, code: [u16; 3]) -> Option<Hit> {
        let position = SamplePosition { x: 0, picture_width: 1 };
        hits_of_code(self, position, code).into_iter().next()
    }

    fn vectorscope_geometry(&self) -> Option<crate::vectorscope_geometry::VectorscopeGeometry> {
        match self {
            Self::VectorscopeYCbCr(rule) => Some(rule.geometry()),
            Self::VectorscopeOklab(rule) => Some(rule.geometry()),
            _ => None,
        }
    }

    /// Радиус полного размаха цвета в клетках (Y'CbCr: ±0,5 с усилением; OKLab: край сетки).
    fn full_range_radius_cells(&self) -> f32 {
        match self {
            Self::VectorscopeYCbCr(rule) => (rule.geometry().half_extent_cells() * f64::from(rule.gain())) as f32,
            Self::VectorscopeOklab(rule) => rule.geometry().half_extent_cells() as f32,
            _ => f32::INFINITY,
        }
    }

    /// Единичное направление линии тона кожи в координатах картинки (x вправо, y вниз).
    fn skin_direction(&self, settings: &RenderSettings) -> Option<[f32; 2]> {
        match self {
            Self::VectorscopeYCbCr(_) => {
                let angle = settings.skin_line_angle_degrees.to_radians();
                Some([angle.cos(), -angle.sin()])
            }
            Self::VectorscopeOklab(rule) => {
                // Тот же путь OKLab, что у следа: положение кода кожи относительно центра.
                let skin_code = settings.skin_tone_srgb.map(srgb_8bit_to_display_code);
                let [column_t, row_t] = rule.grid_position(skin_code);
                let offset = rule.centre_offset();
                let direction = [column_t - offset, row_t - offset];
                let length = direction[0].hypot(direction[1]);
                // Нейтральный «кожный» цвет (длина 0) направления не имеет: линию не рисуем.
                if length > 0.0 {
                    Some([direction[0] / length, direction[1] / length])
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

fn code_gray(code: u16) -> [u16; 3] {
    [code; 3]
}

/// 8-битный код sRGB → 10-битный код экрана (то же, что видоискатель получил бы при показе этого цвета).
fn srgb_8bit_to_display_code(value: u8) -> u16 {
    (f32::from(value) / 255.0 * f32::from(MAX_DISPLAY_CODE)).round() as u16
}
