// Правило диаграммы цветности CIE: `scope_hits`. Подключается ПОСЛЕ `scope_accumulate_common.glsl`.
// CPU-эталон: `scope_model::cie_rule` (цепочка) и `scope_model::cie_cell` (точное деление в ячейку).
// Слова прибора: 4 — предел сетки size (float); 5 — клеток на единицу шкалы; 6 — левая граница окна; 7 — верхняя
// граница окна; 8 — порог яркости Y; 9..11 — строка числителя столбца; 12..14 — строка числителя строки;
// 15..17 — строка знаменателя; 18..20 — строка яркости Y (каждая строка — веса при линейных R, G, B экрана).
// Шкала (xy 1931 или u'v' 1976) целиком в строках матрицы: ветвления по шкале в шейдере нет.
//
// ТОЧНОЕ ДЕЛЕНИЕ. Деление на GPU не округляется точно, поэтому ячейка по оси определяется только умножениями:
// наибольшее целое c >= 0 с fl(c * S) <= t. Приближение floor(t * (1 / S)) ошибается не больше чем на клетку, а одна
// поправка в каждую сторону даёт точный ответ (см. `cie_cell.rs`).
const uint WORD_CIE_SIZE_LIMIT = WORD_RULE_FIRST;
const uint WORD_CIE_CELLS_PER_UNIT = WORD_RULE_FIRST + 1u;
const uint WORD_CIE_WINDOW_LEFT = WORD_RULE_FIRST + 2u;
const uint WORD_CIE_WINDOW_TOP = WORD_RULE_FIRST + 3u;
const uint WORD_CIE_MIN_LUMINANCE = WORD_RULE_FIRST + 4u;
const uint WORD_CIE_COLUMN_ROW = WORD_RULE_FIRST + 5u;
const uint WORD_CIE_ROW_ROW = WORD_RULE_FIRST + 8u;
const uint WORD_CIE_DENOMINATOR_ROW = WORD_RULE_FIRST + 11u;
const uint WORD_CIE_LUMINANCE_ROW = WORD_RULE_FIRST + 14u;

// `(r0*v0 + r1*v1) + r2*v2` — единственный порядок суммирования (как `dot_in_fixed_order` на CPU).
float cie_dot_in_fixed_order(uint first_word, vec3 vector) {
    precise float term_r = word_as_float(first_word) * vector.x;
    precise float term_g = word_as_float(first_word + 1u) * vector.y;
    precise float head = term_r + term_g;
    precise float term_b = word_as_float(first_word + 2u) * vector.z;
    precise float sum = head + term_b;
    return sum;
}

// Ячейка по оси или -1 (вне сетки). `numerator` — t, `denominator` — S > 0.
int cie_cell_on_axis(float numerator, float denominator, float size_limit) {
    // Отрицание сравнения: NaN тоже вне сетки.
    if (!(numerator >= 0.0)) {
        return -1;
    }
    precise float reciprocal = 1.0 / denominator;
    precise float quotient = numerator * reciprocal;
#if defined(SCOPE_CIE_GUESS_SKEW)
    // ДИАГНОСТИКА: приближение сдвинуто так, что оно часто ошибается на целую клетку; точность ответа обязана
    // остаться (поправка). Только в тестовых шейдерах `scope_diag_cie_*`.
    quotient += SCOPE_CIE_GUESS_SKEW;
    quotient = max(quotient, 0.0);
#endif
    // Приближение не больше size + 1: дальше всё равно «вне сетки».
    float guess = min(floor(quotient), size_limit + 1.0);
    precise float guess_product = guess * denominator;
    precise float next_guess = guess + 1.0;
    precise float next_product = next_guess * denominator;
    float cell = guess;
    if (guess_product > numerator) {
        cell = guess - 1.0;
    } else if (next_product <= numerator) {
        cell = guess + 1.0;
    }
    if (!(cell < size_limit)) {
        return -1;
    }
    return int(cell);
}

void scope_hits(uint eye_index, uvec3 code) {
    // Код -> свет экрана по таблице (те же числа, что на CPU).
    vec3 linear = vec3(
        parameters.decode_table[code.x],
        parameters.decode_table[code.y],
        parameters.decode_table[code.z]);
    float denominator = cie_dot_in_fixed_order(WORD_CIE_DENOMINATOR_ROW, linear);
    float luminance = cie_dot_in_fixed_order(WORD_CIE_LUMINANCE_ROW, linear);
    // Не число, неположительный знаменатель (чёрный) и тёмный шум — без попадания.
    if (!(denominator > 0.0) || luminance < word_as_float(WORD_CIE_MIN_LUMINANCE)) {
        return;
    }
    float column_numerator = cie_dot_in_fixed_order(WORD_CIE_COLUMN_ROW, linear);
    float row_numerator = cie_dot_in_fixed_order(WORD_CIE_ROW_ROW, linear);
    float cells_per_unit = word_as_float(WORD_CIE_CELLS_PER_UNIT);
    precise float left_scaled = word_as_float(WORD_CIE_WINDOW_LEFT) * denominator;
    precise float shifted_column = column_numerator - left_scaled;
    precise float column_t = shifted_column * cells_per_unit;
    precise float top_scaled = word_as_float(WORD_CIE_WINDOW_TOP) * denominator;
    precise float shifted_row = top_scaled - row_numerator;
    precise float row_t = shifted_row * cells_per_unit;
    float size_limit = word_as_float(WORD_CIE_SIZE_LIMIT);
    int column = cie_cell_on_axis(column_t, denominator, size_limit);
    int row = cie_cell_on_axis(row_t, denominator, size_limit);
    if (column < 0 || row < 0) {
        return;
    }
    scope_add_hit(0u, uint(row), uint(column));
}
