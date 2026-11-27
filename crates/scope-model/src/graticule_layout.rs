//! Раскладка сетки прибора: из решённого правила и настроек — список [`GraticulePrimitive`].
//!
//! Положения находятся ЧЕРЕЗ ТО ЖЕ правило попадания, что и след: линия уровня — там, куда правило кладёт
//! серый код этого уровня; цель цвета — там, куда оно кладёт код цвета. Поэтому сетка не расходится со
//! следом ни при какой системе осей.

use crate::codes::MAX_DISPLAY_CODE;
use crate::graticule::{GraticulePrimitive, MAX_GRATICULE_PRIMITIVES};
use crate::histogram_strips::HistogramStrips;
use crate::render_settings::RenderSettings;
use crate::resolved::ResolvedScope;
use crate::rule::{Hit, SamplePosition, hits_of_code};
use scope_contract::{HistogramMode, WAVEFORM_LEVEL_LINES};

/// Цвета целей вектороскопа: R, G, B, C, M, Y (каналы 0/1 умножаются на амплитуду).
const TARGET_COLORS: [[f32; 3]; 6] = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 1.0, 1.0],
    [1.0, 0.0, 1.0],
    [1.0, 1.0, 0.0],
];

/// Шагов сетки гистограммы по диапазону кодов: 10 шагов — линии через каждые 10 % (решение 337).
const HISTOGRAM_GRID_STEPS: u32 = 10;

/// Длина плеча креста центра в полуразмерах рамки цели.
const CROSS_ARM_IN_TARGET_HALVES: f32 = 2.0;

impl ResolvedScope {
    /// Сетка прибора. Число примитивов не больше [`MAX_GRATICULE_PRIMITIVES`] (проверяется тестом).
    pub fn graticule(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        let mut primitives = match self {
            // У наложенного RGB waveform те же пять уровней, что у parade: сетка не зависит от способа рисования.
            Self::Histogram(_) => self.histogram_level_lines(settings),
            Self::Waveform(_) | Self::LumaWaveform(_) | Self::RgbParade(_) => self.level_lines(settings),
            Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) => self.vectorscope_marks(settings),
            Self::Cie(rule) => crate::cie_graticule::cie_graticule(rule, settings),
        };
        primitives.truncate(MAX_GRATICULE_PRIMITIVES);
        primitives
    }

    /// Горизонтальные линии уровней 0, 25, 50, 75, 100 %.
    fn level_lines(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        // `match` без `_`: новый прибор не соберётся, пока не скажет, есть ли у него линии уровней.
        let rows = match self {
            Self::Waveform(rule) | Self::RgbParade(rule) => rule.levels(),
            Self::LumaWaveform(rule) => rule.levels(),
            Self::Histogram(_) | Self::VectorscopeYCbCr(_) | Self::VectorscopeOklab(_) | Self::Cie(_) => {
                return Vec::new();
            }
        };
        // Уровни — общий список словаря: экран подписывает именно эти линии (решения 188, 274).
        WAVEFORM_LEVEL_LINES
            .iter()
            .filter_map(|line| {
                let code = (line.fraction * f32::from(MAX_DISPLAY_CODE)).round() as u16;
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

    /// Сетка гистограммы (решение 337): вертикальные линии через каждые 10 % диапазона кодов (0, 10, …, 100 % — 11 линий;
    /// 50 % толще прочих) и базовая линия каждой полосы PARADE. x линии — центр столбца, куда правило кладёт серый код
    /// этого уровня (не номинальные доли ширины), поэтому линия и столбик серого кадра совпадают пиксель в пиксель.
    /// Полосы лежат друг над другом на всю ширину, поэтому линия каждой полосы идёт только по её высоте (в зазоре между
    /// полосами сетки нет), а базовые линии отделяют полосы. У RGB и Y полоса одна на всю высоту, базовой линии нет
    /// (это нижний край картинки).
    fn histogram_level_lines(&self, settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        let Self::Histogram(rule) = self else {
            return Vec::new();
        };
        let strip_count = match rule.mode() {
            HistogramMode::Parade => rule.level_projections().len() as u32,
            HistogramMode::Rgb | HistogramMode::Luma => 1,
        };
        let strips = HistogramStrips::new(strip_count, settings.histogram_strip_gap);
        let mut lines = Vec::new();
        for strip in 0..strips.count() {
            let (top, bottom) = strips.normalized_span(strip);
            for step in 0..=HISTOGRAM_GRID_STEPS {
                let fraction = step as f32 / HISTOGRAM_GRID_STEPS as f32;
                let code = (fraction * f32::from(MAX_DISPLAY_CODE)).round() as u16;
                let Some(hit) = self.first_cell_of(code_gray(code)) else {
                    continue;
                };
                let x = (hit.column as f32 + 0.5) / rule.bins() as f32;
                let is_midline = step * 2 == HISTOGRAM_GRID_STEPS;
                lines.push(GraticulePrimitive::Segment {
                    from: [x, top],
                    to: [x, bottom],
                    half_width_px: if is_midline {
                        settings.histogram_midline_half_width_px
                    } else {
                        settings.graticule_half_width_px
                    },
                });
            }
            if strips.count() > 1 {
                lines.push(GraticulePrimitive::Segment {
                    from: [0.0, bottom],
                    to: [1.0, bottom],
                    half_width_px: settings.graticule_half_width_px,
                });
            }
        }
        lines
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
                // Линия кожи — главный ориентир: своя полуширина, толще прочей сетки.
                half_width_px: settings.skin_line_half_width_px,
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
        // Цели: полосы 75 % (крупные рамки) и полный размах 100 % (мелкие) — какие из них, решают настройки.
        let targets = settings.vectorscope_targets;
        let target_sets = [
            (
                targets.draws_bars(),
                settings.target_amplitude,
                settings.target_box_half_cells,
            ),
            (targets.draws_full(), 1.0, settings.full_target_box_half_cells),
        ];
        for (drawn, amplitude, box_half_cells) in target_sets {
            if !drawn {
                continue;
            }
            let half_size = box_half_cells / size;
            for color in TARGET_COLORS {
                // Цель вне сетки (большое усиление) просто не рисуется: следа в этом месте тоже быть не может.
                if let Some(hit) = self.target_cell(color, amplitude) {
                    primitives.push(GraticulePrimitive::BoxOutline {
                        centre: [(hit.column as f32 + 0.5) / size, (hit.row as f32 + 0.5) / size],
                        half_size: [half_size, half_size],
                        half_width_px,
                    });
                }
            }
        }
        primitives
    }

    /// Ячейка цели цвета `channels` (0/1 на канал, как в [`TARGET_COLORS`]) при амплитуде `amplitude`: туда правило
    /// кладёт код такого цвета. `None` — цель вне сетки. Единственный путь и для рамок сетки, и для подписей целей.
    pub(crate) fn target_cell(&self, channels: [f32; 3], amplitude: f32) -> Option<Hit> {
        let amplitude_code = (amplitude * f32::from(MAX_DISPLAY_CODE)).round() as u16;
        let code = channels.map(|channel| if channel > 0.0 { amplitude_code } else { 0 });
        self.first_cell_of(code)
    }

    /// Первая ячейка, куда правило кладёт код (отсчёт в начале картинки ширины 1).
    fn first_cell_of(&self, code: [u16; 3]) -> Option<Hit> {
        let position = SamplePosition {
            eye_index: 0,
            eye_samples: 1,
        };
        hits_of_code(self, position, code).into_iter().next()
    }

    pub(crate) fn vectorscope_geometry(&self) -> Option<crate::vectorscope_geometry::VectorscopeGeometry> {
        match self {
            Self::VectorscopeYCbCr(coloured) => Some(coloured.rule().geometry()),
            Self::VectorscopeOklab(coloured) => Some(coloured.rule().geometry()),
            _ => None,
        }
    }

    /// Радиус кольца в клетках: граница круга картинки на обеих осях (решение 324).
    fn full_range_radius_cells(&self) -> f32 {
        match self {
            // Кольцо — граница круга картинки (решение 324): цели и след масштабируются внутри него.
            Self::VectorscopeYCbCr(coloured) => coloured.rule().geometry().half_extent_cells() as f32,
            Self::VectorscopeOklab(coloured) => coloured.rule().geometry().half_extent_cells() as f32,
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
            Self::VectorscopeOklab(coloured) => {
                let rule = coloured.rule();
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
