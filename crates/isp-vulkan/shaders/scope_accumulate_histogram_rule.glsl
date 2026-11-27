// Правило гистограммы (сессия 14s): `scope_hits`. Подключается ПОСЛЕ `scope_accumulate_common.glsl`.
// CPU-эталон: `scope_model::histogram_rule` (`HistogramRule`). Сетка — `bins` столбцов, ОДНА строка, плоскости: R', G', B'
// (режимы RGB и PARADE) или одна плоскость яркости Y' (Luma). Попадание плоскости: ячейка (плоскость, строка 0, столбец =
// проекция кода этой плоскости). Положение отсчёта на картинке гистограмме безразлично (`eye_index` не читается).
// Слова прибора (после общего заголовка):
//   4..18  три проекции «код → уровень» по плоскостям: по 5 слов `w0 w1 w2 offset shift`; плоскости сверх сетки не читаются.
// Проекция и её слова — общие с правилом waveform (`scope_accumulate_projection.glsl`).
#include "scope_accumulate_projection.glsl"

const uint WORD_LEVEL_PROJECTIONS = WORD_RULE_FIRST;

// Слова правила читаются ОДИН раз за нить (при первом отсчёте), как у `scope_accumulate_integer_rule.glsl`.
bool rule_loaded = false;
Projection level_projection_0;
Projection level_projection_1;
Projection level_projection_2;

void load_rule_words() {
    level_projection_0 = projection_at(WORD_LEVEL_PROJECTIONS);
    level_projection_1 = projection_at(WORD_LEVEL_PROJECTIONS + PROJECTION_WORDS);
    level_projection_2 = projection_at(WORD_LEVEL_PROJECTIONS + 2u * PROJECTION_WORDS);
    rule_loaded = true;
}

// Попадание плоскости `plane` в уровень её проекции (плоскости развёрнуты явно, без цикла и выбора по индексу).
void hit_level(uint plane, Projection projection, ivec3 code) {
    int level = projection_value(projection, code);
    if (level < 0 || level >= int(grid_columns)) {
        return;
    }
    scope_add_hit(plane, 0u, uint(level));
}

void scope_hits(uint eye_index, uvec3 code) {
    if (!rule_loaded) {
        load_rule_words();
    }
    ivec3 signed_code = ivec3(code);
    hit_level(0u, level_projection_0, signed_code);
    if (grid_planes > 1u) {
        hit_level(1u, level_projection_1, signed_code);
    }
    if (grid_planes > 2u) {
        hit_level(2u, level_projection_2, signed_code);
    }
}
