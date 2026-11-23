// Общий блок основы тона на GPU (решение 182): ОДИН uniform-блок `ToneBaseParameters` под любую модель основы
// (прежняя, AgX, дальше ACES, filmic…). Это только «хранилище»: блок — массив строк `vec4`, а что лежит в строках
// модели, знает её собственный GLSL-файл (`tone_base_<модель>.glsl`) и её Rust-упаковщик
// (`src/tone_base/<модель>.rs`). Файл ничего не знает ни о каких моделях, кроме номеров ([`TONE_MODEL_*`]).
// Как добавить модель — `src/tone_base/mod.rs`.
//
// Включающий файл ДО #include определяет `TONE_BASE_PARAMS_BINDING` — номер привязки (set 0).
//
// Раскладка (зеркало `src/tone_base/mod.rs`, менять только вместе):
//   строка 0 — общая для всех моделей: xyz — усиления баланса белого R, G, B (для белого точки `L`),
//              w — номер модели основы (0.0 прежняя, 1.0 AgX, 2.0 ACES…; целое в float — точно);
//   строки 1… — числа выбранной модели (см. её файл); неиспользуемые строки — нули.
// Флаги модели лежат ЧИСЛАМИ float (0.0 / 1.0), а не uint: чтение uint-битов из vec4 (floatBitsToUint) на
// денормализованных значениях небезопасно (драйвер вправе сбросить их в нуль).
//
// Файл защищён от повторного включения.
#ifndef TONE_BASE_GLSL
#define TONE_BASE_GLSL

#include "color_tail.glsl"

// Наибольшее число строк блока (зеркало `TONE_BASE_ROWS_MAX` в Rust): 64 · 16 = 1 КиБ, с запасом под самую
// жирную модель (AgX с возвратом оттенка занимает 34 строки) и сильно ниже минимума 16 КиБ, который
// гарантирует Vulkan для uniform-блока.
#define TONE_BASE_ROWS_MAX 64

// Номера моделей основы (зеркало `ToneBaseKind::model_id` в Rust).
const uint TONE_MODEL_LEGACY = 0u;
const uint TONE_MODEL_AGX = 1u;
const uint TONE_MODEL_ACES = 2u;
const uint TONE_MODEL_FILMIC = 3u;

// Первая строка чисел модели (строка 0 — общая).
const uint TONE_BASE_FIRST_MODEL_ROW = 1u;

layout(set = 0, binding = TONE_BASE_PARAMS_BINDING, std140) uniform ToneBaseParameters {
    vec4 rows[TONE_BASE_ROWS_MAX];
} tone_base;

// Усиления баланса белого по цветам R, G, B (для своего белого точки; нужны всем моделям с `PerPoint`).
vec3 tone_base_white_balance() {
    return tone_base.rows[0].xyz;
}

// Номер модели основы кадра. Видоискатель выбирает модель константой специализации (вариант шейдера), а
// качественный путь и снимок читают номер отсюда на лету.
uint tone_base_model() {
    return uint(tone_base.rows[0].w + 0.5);
}

// Строка матрицы · пиксель слева направо без FMA — как PixelMatrix::apply.
float tone_base_matrix_row(vec4 row, vec3 pixel) {
    precise float value = (row.x * pixel.x + row.y * pixel.y) + row.z * pixel.z;
    return value;
}

// Матрица 3×3, лежащая тремя строками блока с `first_row`, · пиксель.
vec3 tone_base_apply_matrix(uint first_row, vec3 pixel) {
    return vec3(
        tone_base_matrix_row(tone_base.rows[first_row], pixel),
        tone_base_matrix_row(tone_base.rows[first_row + 1u], pixel),
        tone_base_matrix_row(tone_base.rows[first_row + 2u], pixel));
}

#endif
