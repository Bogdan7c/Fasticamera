//! Упаковка блоков параметров в байты буферов шейдеров (раскладка std430, как в шейдерах).
//!
//! Две раскладки:
//! * блок накопления — `uint words[64]` (заголовок: столбцы, строки, плоскости, чтение картинки глазом; потом слова прибора)
//!   и `float decode_table[1024]` (`scope_accumulate_common.glsl`);
//! * блок показа — сетка и число примитивов, непрозрачности, код способа рисования, цвета плоскостей, 48
//!   примитивов и ХВОСТ «цвет по положению» (таблица углов, нейтраль, геометрия; нули у всех способов, кроме
//!   `TracePaint::ColorByPosition`) (`scope_render.comp`).
//!
//! Чистые функции без GPU: раскладку проверяют обычные тесты.

use scope_contract::ScopeScan;
use scope_model::position_palette::{MIN_EDGE_RADIUS_CELLS, NEUTRAL_RADIUS_EPSILON_CELLS};
use scope_model::{
    GraticulePrimitive, GridShape, MAX_GRATICULE_PRIMITIVES, POSITION_PALETTE_ANGLES, PositionPalette, RenderSettings,
    TracePaint,
};

use super::error::ScopeError;
use super::rule::{AccumulateParameters, DECODE_TABLE_LEN, HEADER_WORDS, PARAMETER_WORDS, RULE_WORD_CAPACITY};

/// Размер блока накопления, байт: слова + таблица.
pub(super) const ACCUMULATE_BLOCK_BYTES: usize = (PARAMETER_WORDS + DECODE_TABLE_LEN) * 4;

/// Размер части блока без таблицы (слова), байт.
pub(super) const ACCUMULATE_WORDS_BYTES: usize = PARAMETER_WORDS * 4;

/// Сколько плоскостей умеет показать шейдер.
pub(super) const MAX_PLANES: usize = 3;

/// Векторов `vec4` в голове блока показа: сетка, `trace`, цвет сетки, три цвета плоскостей.
const RENDER_HEAD_VECTORS: usize = 6;

/// Векторов `vec4` в хвосте блока показа: таблица углов, нейтраль, геометрия центра.
const RENDER_TAIL_VECTORS: usize = POSITION_PALETTE_ANGLES + 2;

/// Размер блока показа, байт: голова + примитивы по два `vec4` + хвост «цвет по положению».
pub(super) const RENDER_BLOCK_BYTES: usize =
    (RENDER_HEAD_VECTORS + MAX_GRATICULE_PRIMITIVES * 2 + RENDER_TAIL_VECTORS) * 16;

/// Слова способа рисования в `trace.w` (`PAINT_*` в `scope_render.comp`). Сравнение в шейдере — по равенству
/// float, поэтому коды — малые целые, точно представимые.
pub(super) const PAINT_CODE_PLANE_SECTIONS: f32 = 0.0;
pub(super) const PAINT_CODE_PLANES_OVERLAID: f32 = 1.0;
pub(super) const PAINT_CODE_COLOR_BY_POSITION: f32 = 2.0;

/// Слово шейдера для способа рисования. `match` без `_`: новый способ не соберётся без своего кода.
pub(super) fn paint_code(paint: TracePaint) -> f32 {
    match paint {
        TracePaint::PlaneSections => PAINT_CODE_PLANE_SECTIONS,
        TracePaint::PlanesOverlaid => PAINT_CODE_PLANES_OVERLAID,
        TracePaint::ColorByPosition => PAINT_CODE_COLOR_BY_POSITION,
    }
}

/// Коды чтения картинки глазом в слове 3 заголовка (`SCAN_*` в `scope_accumulate_common.glsl`).
pub(super) const SCAN_CODE_UPRIGHT: u32 = 0;
pub(super) const SCAN_CODE_ROTATED_CLOCKWISE: u32 = 1;
pub(super) const SCAN_CODE_UPSIDE_DOWN: u32 = 2;
pub(super) const SCAN_CODE_ROTATED_COUNTER_CLOCKWISE: u32 = 3;

/// Слово шейдера для чтения картинки глазом. `match` без `_`: новый вариант не соберётся без своего кода.
pub(super) fn scan_code(scan: ScopeScan) -> u32 {
    match scan {
        ScopeScan::Upright => SCAN_CODE_UPRIGHT,
        ScopeScan::RotatedClockwise => SCAN_CODE_ROTATED_CLOCKWISE,
        ScopeScan::UpsideDown => SCAN_CODE_UPSIDE_DOWN,
        ScopeScan::RotatedCounterClockwise => SCAN_CODE_ROTATED_COUNTER_CLOCKWISE,
    }
}

/// Слова блока накопления: заголовок (форма сетки, чтение глазом) и слова прибора, добитые нулями до
/// `PARAMETER_WORDS`.
pub(super) fn accumulate_words_bytes(
    grid: GridShape,
    scan: ScopeScan,
    parameters: &AccumulateParameters<'_>,
) -> Result<Vec<u8>, ScopeError> {
    let rule_words = parameters.words.as_slice();
    if rule_words.len() > RULE_WORD_CAPACITY {
        return Err(ScopeError::TooManyRuleWords {
            words: rule_words.len(),
            capacity: RULE_WORD_CAPACITY,
        });
    }
    let mut words = vec![0_u32; PARAMETER_WORDS];
    words[0] = grid.columns();
    words[1] = grid.rows();
    words[2] = grid.planes();
    words[3] = scan_code(scan);
    words[HEADER_WORDS..HEADER_WORDS + rule_words.len()].copy_from_slice(rule_words);
    Ok(words.iter().flat_map(|word| word.to_ne_bytes()).collect())
}

/// Байты таблицы декодирования.
pub(super) fn decode_table_bytes(table: &[f32; DECODE_TABLE_LEN]) -> Vec<u8> {
    table.iter().flat_map(|value| value.to_ne_bytes()).collect()
}

/// Всё, что уходит в блок показа одного кадра.
#[derive(Clone, Copy)]
pub(super) struct RenderBlockInputs<'a> {
    /// Форма сетки счётчиков.
    pub grid: GridShape,
    /// Способ рисования следа.
    pub paint: TracePaint,
    /// Цвета плоскостей: по одному на плоскость, плоскостей не больше `MAX_PLANES` (проверено вызывающим).
    pub colors: &'a [[f32; 3]],
    /// Таблица «цвет по положению»; нужна только способу `ColorByPosition` (проверено вызывающим).
    pub position_palette: Option<&'a PositionPalette>,
    /// Примитивы сетки: не больше `MAX_GRATICULE_PRIMITIVES` (проверено вызывающим).
    pub graticule: &'a [GraticulePrimitive],
    /// Настройки показа.
    pub settings: &'a RenderSettings,
}

/// Блок показа.
pub(super) fn render_block_bytes(inputs: &RenderBlockInputs<'_>) -> Vec<u8> {
    let RenderBlockInputs {
        grid,
        paint,
        colors,
        position_palette,
        graticule,
        settings,
    } = *inputs;
    let mut floats: Vec<u32> = Vec::with_capacity(RENDER_BLOCK_BYTES / 4);
    // uvec4 grid: столбцы, строки, плоскости, число примитивов (в буфере не больше 48: проверено вызывающим).
    floats.extend([grid.columns(), grid.rows(), grid.planes(), graticule.len() as u32]);
    // vec4 trace: знаменатель лог-шкалы (тот же f32, что на CPU), непрозрачность следа и сетки, код способа.
    let saturation_denominator = (1.0 + settings.trace_saturation_count as f32).log2();
    floats.extend(
        [
            saturation_denominator,
            settings.trace_opacity,
            settings.graticule_opacity,
            paint_code(paint),
        ]
        .map(f32::to_bits),
    );
    floats.extend(rgb_with_padding(settings.graticule_color));
    for plane in 0..MAX_PLANES {
        floats.extend(rgb_with_padding(colors.get(plane).copied().unwrap_or([0.0; 3])));
    }
    for index in 0..MAX_GRATICULE_PRIMITIVES {
        floats.extend(graticule.get(index).map_or([0; 8], primitive_words));
    }
    floats.extend(position_tail_words(position_palette));
    floats.iter().flat_map(|word| word.to_ne_bytes()).collect()
}

/// Хвост блока: `vec4 position_palette[N]` (rgb края, радиус края), `vec4 position_neutral` (rgb), `vec4
/// position_geometry` (центр, радиус «это центр», наименьший радиус края). Без таблицы — нули.
fn position_tail_words(palette: Option<&PositionPalette>) -> Vec<u32> {
    let Some(palette) = palette else {
        return vec![0; RENDER_TAIL_VECTORS * 4];
    };
    let mut words = Vec::with_capacity(RENDER_TAIL_VECTORS * 4);
    for entry in palette.entries() {
        let [red, green, blue] = entry.edge_colour;
        words.extend([red, green, blue, entry.edge_radius_cells].map(f32::to_bits));
    }
    words.extend(rgb_with_padding(palette.neutral()));
    words.extend(
        [
            palette.centre_cells(),
            NEUTRAL_RADIUS_EPSILON_CELLS,
            MIN_EDGE_RADIUS_CELLS,
            0.0,
        ]
        .map(f32::to_bits),
    );
    words
}

fn rgb_with_padding(color: [f32; 3]) -> [u32; 4] {
    [color[0].to_bits(), color[1].to_bits(), color[2].to_bits(), 0]
}

/// Вид примитива в `style.y`: 0 — отрезок, 1 — кольцо, 2 — рамка.
fn primitive_words(primitive: &GraticulePrimitive) -> [u32; 8] {
    let (geometry, half_width_px, kind) = match *primitive {
        GraticulePrimitive::Segment {
            from,
            to,
            half_width_px,
        } => ([from[0], from[1], to[0], to[1]], half_width_px, 0.0),
        GraticulePrimitive::Ring {
            centre,
            radius,
            half_width_px,
        } => ([centre[0], centre[1], radius[0], radius[1]], half_width_px, 1.0),
        GraticulePrimitive::BoxOutline {
            centre,
            half_size,
            half_width_px,
        } => ([centre[0], centre[1], half_size[0], half_size[1]], half_width_px, 2.0),
    };
    let [a, b, c, d] = geometry;
    [a, b, c, d, half_width_px, kind, 0.0, 0.0].map(f32::to_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::rule::RuleWords;

    fn words_of(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    #[test]
    fn accumulate_block_has_grid_header_then_rule_words_then_zero_padding() {
        let grid = GridShape::try_new(7, 5, 3).unwrap();
        let mut rule_words = RuleWords::new();
        rule_words.push_u32(11).push_f32(2.0);
        let parameters = AccumulateParameters {
            words: rule_words,
            decode_table: None,
        };
        let bytes = accumulate_words_bytes(grid, ScopeScan::Upright, &parameters).unwrap();
        assert_eq!(bytes.len(), ACCUMULATE_WORDS_BYTES);
        let words = words_of(&bytes);
        assert_eq!(&words[..6], &[7, 5, 3, 0, 11, 2.0_f32.to_bits()]);
        assert!(words[6..].iter().all(|&word| word == 0));
    }

    #[test]
    fn scan_goes_to_header_word_three_without_touching_other_words() {
        let grid = GridShape::try_new(7, 5, 3).unwrap();
        let build = |scan| {
            let mut rule_words = RuleWords::new();
            rule_words.push_u32(11);
            let parameters = AccumulateParameters {
                words: rule_words,
                decode_table: None,
            };
            words_of(&accumulate_words_bytes(grid, scan, &parameters).unwrap())
        };
        let upright = build(ScopeScan::Upright);
        for (scan, expected_code) in [
            (ScopeScan::Upright, 0),
            (ScopeScan::RotatedClockwise, 1),
            (ScopeScan::UpsideDown, 2),
            (ScopeScan::RotatedCounterClockwise, 3),
        ] {
            let words = build(scan);
            assert_eq!(words[3], expected_code, "{scan:?}");
            assert_eq!(&words[..3], &upright[..3], "сетка не зависит от чтения");
            assert_eq!(&words[4..], &upright[4..], "слова прибора не зависят от чтения");
        }
    }

    #[test]
    fn too_many_rule_words_are_rejected_not_truncated() {
        let grid = GridShape::try_new(1, 1, 1).unwrap();
        let mut rule_words = RuleWords::new();
        for _ in 0..=RULE_WORD_CAPACITY {
            rule_words.push_u32(1);
        }
        let parameters = AccumulateParameters {
            words: rule_words,
            decode_table: None,
        };
        assert_eq!(
            accumulate_words_bytes(grid, ScopeScan::Upright, &parameters),
            Err(ScopeError::TooManyRuleWords {
                words: RULE_WORD_CAPACITY + 1,
                capacity: RULE_WORD_CAPACITY
            })
        );
    }

    /// Номера слов блока показа (слово = 4 байта): где начинается каждая часть. Единственное место, где тесты
    /// считают раскладку; шейдер повторяет её порядком полей `RenderParameters`.
    const WORD_TRACE: usize = 4;
    const WORD_PLANE_COLORS: usize = 12;
    const WORD_PRIMITIVES: usize = 24;
    const WORD_POSITION_PALETTE: usize = WORD_PRIMITIVES + MAX_GRATICULE_PRIMITIVES * 8;
    const WORD_POSITION_NEUTRAL: usize = WORD_POSITION_PALETTE + POSITION_PALETTE_ANGLES * 4;
    const WORD_POSITION_GEOMETRY: usize = WORD_POSITION_NEUTRAL + 4;

    fn block_of(
        paint: TracePaint,
        palette: Option<&PositionPalette>,
        primitives: &[GraticulePrimitive],
        settings: &RenderSettings,
    ) -> Vec<u32> {
        let grid = GridShape::try_new(4, 3, 2).unwrap();
        let bytes = render_block_bytes(&RenderBlockInputs {
            grid,
            paint,
            colors: &[[1.0, 0.5, 0.25], [0.0, 1.0, 0.0]],
            position_palette: palette,
            graticule: primitives,
            settings,
        });
        assert_eq!(bytes.len(), RENDER_BLOCK_BYTES);
        words_of(&bytes)
    }

    fn test_palette() -> PositionPalette {
        let entries = std::array::from_fn(|index| scope_model::PositionPaletteEntry {
            edge_colour: [index as f32, 0.5, 0.25],
            edge_radius_cells: 100.0 + index as f32,
        });
        PositionPalette::new(entries, [0.7, 0.6, 0.5], 128.5)
    }

    #[test]
    fn render_block_layout_matches_the_shader_struct() {
        let settings = RenderSettings::default();
        let primitive = GraticulePrimitive::Ring {
            centre: [0.5, 0.25],
            radius: [0.125, 0.0625],
            half_width_px: 0.75,
        };
        let words = block_of(TracePaint::PlaneSections, None, &[primitive], &settings);
        assert_eq!(&words[..4], &[4, 3, 2, 1], "сетка и число примитивов");
        assert_eq!(
            f32::from_bits(words[WORD_TRACE]),
            (1.0 + settings.trace_saturation_count as f32).log2()
        );
        assert_eq!(f32::from_bits(words[WORD_TRACE + 1]), settings.trace_opacity);
        assert_eq!(f32::from_bits(words[WORD_TRACE + 2]), settings.graticule_opacity);
        assert_eq!(f32::from_bits(words[WORD_TRACE + 3]), PAINT_CODE_PLANE_SECTIONS);
        // plane_colors[0] начинается после заголовка (4 слова) + trace (4) + graticule_color (4).
        assert_eq!(f32::from_bits(words[WORD_PLANE_COLORS]), 1.0);
        assert_eq!(f32::from_bits(words[WORD_PLANE_COLORS + 1]), 0.5);
        // plane_colors[2] — третьей плоскости нет: нули.
        assert_eq!(&words[20..24], &[0, 0, 0, 0]);
        // Первый примитив: geometry (центр, радиусы), style (полуширина, вид «кольцо»).
        assert_eq!(f32::from_bits(words[WORD_PRIMITIVES]), 0.5);
        assert_eq!(f32::from_bits(words[WORD_PRIMITIVES + 3]), 0.0625);
        assert_eq!(f32::from_bits(words[WORD_PRIMITIVES + 4]), 0.75);
        assert_eq!(f32::from_bits(words[WORD_PRIMITIVES + 5]), 1.0);
        // Остальные примитивы и весь хвост «по положению» пусты (нули): буфер заполнен целиком.
        assert!(words[WORD_PRIMITIVES + 8..].iter().all(|&word| word == 0));
    }

    #[test]
    fn paint_code_goes_to_trace_w_and_is_distinct_for_every_paint() {
        let settings = RenderSettings::default();
        let mut seen = Vec::new();
        for (paint, expected) in [
            (TracePaint::PlaneSections, PAINT_CODE_PLANE_SECTIONS),
            (TracePaint::PlanesOverlaid, PAINT_CODE_PLANES_OVERLAID),
            (TracePaint::ColorByPosition, PAINT_CODE_COLOR_BY_POSITION),
        ] {
            let palette = test_palette();
            let words = block_of(paint, Some(&palette), &[], &settings);
            let code = f32::from_bits(words[WORD_TRACE + 3]);
            assert_eq!(code, expected, "{paint:?}");
            assert!(!seen.contains(&code.to_bits()), "код {paint:?} повторяет чужой");
            seen.push(code.to_bits());
            assert_eq!(paint_code(paint), expected);
        }
        // Код прежнего способа — ровно нулевое слово: прежний блок не отличается от нового ни одним битом.
        assert_eq!(PAINT_CODE_PLANE_SECTIONS.to_bits(), 0);
    }

    #[test]
    fn position_tail_carries_palette_neutral_and_geometry_in_shader_order() {
        let settings = RenderSettings::default();
        let palette = test_palette();
        let words = block_of(TracePaint::ColorByPosition, Some(&palette), &[], &settings);
        for index in [0, 1, 77, POSITION_PALETTE_ANGLES - 1] {
            let at = WORD_POSITION_PALETTE + index * 4;
            let entry = [
                f32::from_bits(words[at]),
                f32::from_bits(words[at + 1]),
                f32::from_bits(words[at + 2]),
                f32::from_bits(words[at + 3]),
            ];
            assert_eq!(entry, [index as f32, 0.5, 0.25, 100.0 + index as f32], "запись {index}");
        }
        let neutral = &words[WORD_POSITION_NEUTRAL..WORD_POSITION_NEUTRAL + 3];
        assert_eq!(
            neutral.iter().map(|&bits| f32::from_bits(bits)).collect::<Vec<_>>(),
            [0.7, 0.6, 0.5]
        );
        let geometry: Vec<f32> = words[WORD_POSITION_GEOMETRY..WORD_POSITION_GEOMETRY + 3]
            .iter()
            .map(|&bits| f32::from_bits(bits))
            .collect();
        assert_eq!(geometry, [128.5, NEUTRAL_RADIUS_EPSILON_CELLS, MIN_EDGE_RADIUS_CELLS]);
        assert_eq!(
            words.len(),
            WORD_POSITION_GEOMETRY + 4,
            "геометрия — последний вектор блока"
        );
    }

    #[test]
    fn position_tail_is_zero_unless_the_paint_supplies_a_palette() {
        let settings = RenderSettings::default();
        let words = block_of(TracePaint::PlanesOverlaid, None, &[], &settings);
        assert!(words[WORD_POSITION_PALETTE..].iter().all(|&word| word == 0));
        // Таблица, выданная чужому способу, всё равно ложится только в хвост: голова блока та же.
        let palette = test_palette();
        let with_palette = block_of(TracePaint::PlanesOverlaid, Some(&palette), &[], &settings);
        assert_eq!(&with_palette[..WORD_POSITION_PALETTE], &words[..WORD_POSITION_PALETTE]);
    }
}
