// Правило целочисленной проекции (waveform, RGB parade, вектороскоп Y'CbCr): `scope_hits`.
// Подключается ПОСЛЕ `scope_accumulate_common.glsl` шейдером накопления и диагностическими вариантами стенда.
// Слова прибора (после общего заголовка):
//   4  источник столбца: 0 — номер отсчёта вдоль горизонтали глаза (waveform, parade; решение 322), 1 — проекция кода;
//   5  основа строки (int);          6  знак строки (int, ±1): row = основа + знак * значение проекции;
//   7..11  проекция столбца: w0 w1 w2 offset shift;
//   12..26 три проекции строки (по плоскостям): по 5 слов, как выше.
#include "scope_accumulate_projection.glsl"

const uint WORD_COLUMN_SOURCE = WORD_RULE_FIRST;
const uint WORD_ROW_BASE = WORD_RULE_FIRST + 1u;
const uint WORD_ROW_SIGN = WORD_RULE_FIRST + 2u;
const uint WORD_COLUMN_PROJECTION = WORD_RULE_FIRST + 3u;
const uint WORD_ROW_PROJECTIONS = WORD_RULE_FIRST + 8u;

// Слова правила читаются ОДИН раз за нить (при первом отсчёте), а не на каждом отсчёте и каждой плоскости (14r,
// разбор цены). Значения те же, порядок целых операций тот же — счётчики бит в бит прежние.
bool rule_loaded = false;
bool column_from_projection = false;
int rule_row_base = 0;
int rule_row_sign = 1;
Projection column_projection;
Projection row_projection_0;
Projection row_projection_1;
Projection row_projection_2;

void load_rule_words() {
    column_from_projection = parameters.words[WORD_COLUMN_SOURCE] != 0u;
    rule_row_base = int(parameters.words[WORD_ROW_BASE]);
    rule_row_sign = int(parameters.words[WORD_ROW_SIGN]);
    column_projection = projection_at(WORD_COLUMN_PROJECTION);
    row_projection_0 = projection_at(WORD_ROW_PROJECTIONS);
    row_projection_1 = projection_at(WORD_ROW_PROJECTIONS + PROJECTION_WORDS);
    row_projection_2 = projection_at(WORD_ROW_PROJECTIONS + 2u * PROJECTION_WORDS);
    rule_loaded = true;
}

// Попадание плоскости `plane` по её проекции строки (плоскости развёрнуты явно, без цикла и выбора по индексу).
void hit_plane(uint plane, Projection projection, ivec3 code, int column) {
    int row = rule_row_base + rule_row_sign * projection_value(projection, code);
    if (row < 0 || row >= int(grid_rows)) {
        return;
    }
    scope_add_hit(plane, uint(row), uint(column));
}

void scope_hits(uint eye_index, uvec3 code) {
    if (!rule_loaded) {
        load_rule_words();
    }
    ivec3 signed_code = ivec3(code);
    int column;
    if (!column_from_projection) {
        // Столбец = номер отсчёта вдоль взгляда; столбцов в кадре ровно столько, сколько таких отсчётов.
        column = int(eye_index);
    } else {
        column = projection_value(column_projection, signed_code);
    }
    if (column < 0 || column >= int(grid_columns)) {
        return;
    }
    hit_plane(0u, row_projection_0, signed_code, column);
    if (grid_planes > 1u) {
        hit_plane(1u, row_projection_1, signed_code, column);
    }
    if (grid_planes > 2u) {
        hit_plane(2u, row_projection_2, signed_code, column);
    }
}
