//! Упаковка блоков параметров в байты буферов шейдеров (раскладка std430, как в шейдерах).
//!
//! Две раскладки:
//! * блок накопления — `uint words[64]` (заголовок: столбцы, строки, плоскости, резерв; потом слова прибора)
//!   и `float decode_table[1024]` (`scope_accumulate_common.glsl`);
//! * блок показа — сетка и число примитивов, непрозрачности, цвета плоскостей, 48 примитивов
//!   (`scope_render.comp`).
//!
//! Чистые функции без GPU: раскладку проверяют обычные тесты.

use scope_model::{GraticulePrimitive, GridShape, MAX_GRATICULE_PRIMITIVES, RenderSettings};

use super::error::ScopeError;
use super::rule::{AccumulateParameters, DECODE_TABLE_LEN, HEADER_WORDS, PARAMETER_WORDS, RULE_WORD_CAPACITY};

/// Размер блока накопления, байт: слова + таблица.
pub(super) const ACCUMULATE_BLOCK_BYTES: usize = (PARAMETER_WORDS + DECODE_TABLE_LEN) * 4;

/// Размер части блока без таблицы (слова), байт.
pub(super) const ACCUMULATE_WORDS_BYTES: usize = PARAMETER_WORDS * 4;

/// Сколько плоскостей умеет показать шейдер.
pub(super) const MAX_PLANES: usize = 3;

/// Размер блока показа, байт: 6 векторов `vec4` заголовка + примитивы по два `vec4`.
pub(super) const RENDER_BLOCK_BYTES: usize = (6 + MAX_GRATICULE_PRIMITIVES * 2) * 16;

/// Слова блока накопления: заголовок формы сетки и слова прибора, добитые нулями до `PARAMETER_WORDS`.
pub(super) fn accumulate_words_bytes(
    grid: GridShape,
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
    words[HEADER_WORDS..HEADER_WORDS + rule_words.len()].copy_from_slice(rule_words);
    Ok(words.iter().flat_map(|word| word.to_ne_bytes()).collect())
}

/// Байты таблицы декодирования.
pub(super) fn decode_table_bytes(table: &[f32; DECODE_TABLE_LEN]) -> Vec<u8> {
    table.iter().flat_map(|value| value.to_ne_bytes()).collect()
}

/// Блок показа. Число цветов равно числу плоскостей (проверено вызывающим), плоскостей не больше `MAX_PLANES`.
pub(super) fn render_block_bytes(
    grid: GridShape,
    colors: &[[f32; 3]],
    graticule: &[GraticulePrimitive],
    settings: &RenderSettings,
) -> Vec<u8> {
    let mut floats: Vec<u32> = Vec::with_capacity(RENDER_BLOCK_BYTES / 4);
    // uvec4 grid: столбцы, строки, плоскости, число примитивов (в буфере не больше 48: проверено вызывающим).
    floats.extend([grid.columns(), grid.rows(), grid.planes(), graticule.len() as u32]);
    // vec4 trace: знаменатель лог-шкалы (тот же f32, что на CPU), непрозрачность следа и сетки.
    let saturation_denominator = (1.0 + settings.trace_saturation_count as f32).log2();
    floats.extend(
        [
            saturation_denominator,
            settings.trace_opacity,
            settings.graticule_opacity,
            0.0,
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
    floats.iter().flat_map(|word| word.to_ne_bytes()).collect()
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
        let bytes = accumulate_words_bytes(grid, &parameters).unwrap();
        assert_eq!(bytes.len(), ACCUMULATE_WORDS_BYTES);
        let words = words_of(&bytes);
        assert_eq!(&words[..6], &[7, 5, 3, 0, 11, 2.0_f32.to_bits()]);
        assert!(words[6..].iter().all(|&word| word == 0));
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
            accumulate_words_bytes(grid, &parameters),
            Err(ScopeError::TooManyRuleWords {
                words: RULE_WORD_CAPACITY + 1,
                capacity: RULE_WORD_CAPACITY
            })
        );
    }

    #[test]
    fn render_block_layout_matches_the_shader_struct() {
        let grid = GridShape::try_new(4, 3, 2).unwrap();
        let settings = RenderSettings::default();
        let primitive = GraticulePrimitive::Ring {
            centre: [0.5, 0.25],
            radius: [0.125, 0.0625],
            half_width_px: 0.75,
        };
        let bytes = render_block_bytes(grid, &[[1.0, 0.5, 0.25], [0.0, 1.0, 0.0]], &[primitive], &settings);
        assert_eq!(bytes.len(), RENDER_BLOCK_BYTES);
        let words = words_of(&bytes);
        assert_eq!(&words[..4], &[4, 3, 2, 1], "сетка и число примитивов");
        assert_eq!(
            f32::from_bits(words[4]),
            (1.0 + settings.trace_saturation_count as f32).log2()
        );
        assert_eq!(f32::from_bits(words[5]), settings.trace_opacity);
        assert_eq!(f32::from_bits(words[6]), settings.graticule_opacity);
        // plane_colors[0] начинается после заголовка (4 слова) + trace (4) + graticule_color (4).
        assert_eq!(f32::from_bits(words[12]), 1.0);
        assert_eq!(f32::from_bits(words[13]), 0.5);
        // plane_colors[2] — третьей плоскости нет: нули.
        assert_eq!(&words[20..24], &[0, 0, 0, 0]);
        // Первый примитив: geometry (центр, радиусы), style (полуширина, вид «кольцо»).
        assert_eq!(f32::from_bits(words[24]), 0.5);
        assert_eq!(f32::from_bits(words[27]), 0.0625);
        assert_eq!(f32::from_bits(words[28]), 0.75);
        assert_eq!(f32::from_bits(words[29]), 1.0);
        // Остальные примитивы пусты (нули), буфер заполнен целиком.
        assert!(words[32..].iter().all(|&word| word == 0));
    }
}
