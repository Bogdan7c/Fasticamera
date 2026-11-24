// Правило целочисленной проекции (waveform, RGB parade, вектороскоп Y'CbCr): `scope_hits`.
// Подключается ПОСЛЕ `scope_accumulate_common.glsl` шейдером накопления и диагностическими вариантами стенда.
// Слова прибора (после общего заголовка):
//   4  источник столбца: 0 — положение отсчёта на горизонтали глаза `(x * columns) / extent` (waveform, parade), 1 — проекция кода;
//   5  основа строки (int);          6  знак строки (int, ±1): row = основа + знак * значение проекции;
//   7..11  проекция столбца: w0 w1 w2 offset shift;
//   12..26 три проекции строки (по плоскостям): по 5 слов, как выше.
const uint WORD_COLUMN_SOURCE = WORD_RULE_FIRST;
const uint WORD_ROW_BASE = WORD_RULE_FIRST + 1u;
const uint WORD_ROW_SIGN = WORD_RULE_FIRST + 2u;
const uint WORD_COLUMN_PROJECTION = WORD_RULE_FIRST + 3u;
const uint WORD_ROW_PROJECTIONS = WORD_RULE_FIRST + 8u;
const uint PROJECTION_WORDS = 5u;

// `(w0*c0 + w1*c1 + w2*c2 + offset) >> shift`, всё в int; `>>` над знаковым числом арифметический.
int projection_value(uint first_word, ivec3 code) {
    int weight_r = int(parameters.words[first_word]);
    int weight_g = int(parameters.words[first_word + 1u]);
    int weight_b = int(parameters.words[first_word + 2u]);
    int offset = int(parameters.words[first_word + 3u]);
    int shift = int(parameters.words[first_word + 4u]);
    return (weight_r * code.x + weight_g * code.y + weight_b * code.z + offset) >> shift;
}

void scope_hits(uint eye_x, uvec3 code) {
    int columns = int(parameters.words[WORD_GRID_COLUMNS]);
    int rows = int(parameters.words[WORD_GRID_ROWS]);
    uint planes = parameters.words[WORD_GRID_PLANES];
    ivec3 signed_code = ivec3(code);

    int column;
    if (parameters.words[WORD_COLUMN_SOURCE] == 0u) {
        // x < 2^14, columns <= 2^12: произведение помещается в uint.
        column = int((eye_x * uint(columns)) / eye_extent);
    } else {
        column = projection_value(WORD_COLUMN_PROJECTION, signed_code);
    }
    if (column < 0 || column >= columns) {
        return;
    }
    int row_base = int(parameters.words[WORD_ROW_BASE]);
    int row_sign = int(parameters.words[WORD_ROW_SIGN]);
    for (uint plane = 0u; plane < planes; ++plane) {
        int value = projection_value(WORD_ROW_PROJECTIONS + plane * PROJECTION_WORDS, signed_code);
        int row = row_base + row_sign * value;
        if (row < 0 || row >= rows) {
            continue;
        }
        scope_add_hit(plane, uint(row), uint(column));
    }
}
