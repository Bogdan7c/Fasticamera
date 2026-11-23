// Слой вида на GPU (решения 98, 99, 176, 178): цвет вида (насыщенность, сочность, полосы оттенка в
// OKLab/OKLCh) и таблица вида (LUT). ОДНА реализация для всех путей: слитый проход видоискателя и выходной
// проход качественного конвейера и снимка. Эталон — isp-model `look/color.rs`, `look/bands.rs`, `look/lut.rs`
// (там точные формулы и порядок операций); числа приходят готовыми из `src/look/mod.rs`.
//
// Слой работает над светом экрана ПОСЛЕ основы тона (любой модели) в рабочем пространстве W и до перевода в цвета
// выхода. Включающий файл ДО #include определяет `LOOK_PARAMS_BINDING` и `LOOK_LUT_BINDING` (set 0) и сам решает,
// вызывать ли стадии; функции стадий нейтраль/«нет таблицы» проверяют сами (по флагу в uniform и заголовку буфера).
//
// Раскладка блока `LookParameters` — зеркало `src/look/mod.rs` (менять только вместе); раскладка буфера таблицы —
// зеркало `src/look/lut_buffer.rs`.
//
// Правила файла:
//   * Нейтраль — ФЛАГ в uniform (`look.rows[0].x`), а не константа специализации: смена варианта pipeline на
//     первом касании ручки создала бы его посреди жеста.
//   * Никаких жёстких порогов около серого: влияние полос сходит на нет гладким весом `c²/(c² + F²)`. Единственная
//     проверка — защита от неопределённого `atan(0, 0)` (в GLSL результат не определён) при ТОЧНО нулевой
//     цветности, где вес полос и так нуль, поэтому на результат она не влияет.
//   * Матрицы OKLab считаются без FMA (`precise`, `oklab_working.glsl`).
//   * Включать только один раз на шейдер.
#ifndef LOOK_STAGE_GLSL
#define LOOK_STAGE_GLSL

#include "color_tail.glsl"
#include "oklab_working.glsl"

// Число строк блока (зеркало `LOOK_ROWS` в Rust).
#define LOOK_ROWS 36

layout(set = 0, binding = LOOK_PARAMS_BINDING, std140) uniform LookParameters {
    vec4 rows[LOOK_ROWS];
} look;

// Таблица: заголовок (x — узлов на ось, 0 — таблицы нет) и узлы подряд по три числа (R, G, B).
layout(set = 0, binding = LOOK_LUT_BINDING, std430) readonly buffer LookLutNodes {
    uvec4 header;
    float values[];
} look_lut;

const uint LOOK_ROW_FLAGS = 0u;        // x — цвет включён, y — занято полос
const uint LOOK_ROW_CHROMA = 1u;       // M_sat, k_vib·range, c_ref, F
const uint LOOK_ROW_SKIN = 2u;         // cos h0, sin h0, cos w, p
const uint LOOK_ROW_OKLAB = 3u;        // 12 строк: W→LMS, LMS'→Lab, Lab→LMS', LMS→W
const uint LOOK_ROW_BAND_CENTERS = 15u;
const uint LOOK_ROW_BAND_HUE_SHIFT = 19u;
const uint LOOK_ROW_BAND_CHROMA = 23u;
const uint LOOK_ROW_BAND_LIGHTNESS = 27u;
const uint LOOK_ROW_WHEELS = 31u;      // x — колёса включены, y — L_g, z — 1/L_g, w — 1/(1 − L_g); 32–35: lift, gamma, gain, offset (ΔL, Δa, Δb)
const uint LOOK_MAX_BANDS = 16u;

// Малая добавка в знаменатель направления кожи (для c → 0), не порог.
const float LOOK_DIRECTION_EPSILON = 1e-6;
const float LOOK_PI = 3.14159265358979323846;
const float LOOK_TAU = 6.28318530717958647692;

#include "look_wheels.glsl"

// ----- Цвет вида -------------------------------------------------------------------------------------------

// Матрицы OKLab W из строк блока (порядок строк — таблица `src/look/mod.rs`).
OklabMatrices look_oklab() {
    OklabMatrices matrices;
    for (uint row = 0u; row < 3u; ++row) {
        matrices.working_to_lms[row] = look.rows[LOOK_ROW_OKLAB + row];
        matrices.lms_prime_to_lab[row] = look.rows[LOOK_ROW_OKLAB + 3u + row];
        matrices.lab_to_lms_prime[row] = look.rows[LOOK_ROW_OKLAB + 6u + row];
        matrices.lms_to_working[row] = look.rows[LOOK_ROW_OKLAB + 9u + row];
    }
    return matrices;
}

// Число `index` из массива полос, лежащего четырьмя строками с `first_row` (по четыре числа в строке).
float look_band_value(uint first_row, uint index) {
    return look.rows[first_row + index / 4u][index % 4u];
}

// Вес защиты кожи 0…1 для направления (a, b): гладкая ступенька по косинусу угла до центра окна
// (LookColorParams::skin_window).
float look_skin_window(float a, float b, float chroma_squared) {
    vec4 skin = look.rows[LOOK_ROW_SKIN];
    float epsilon_squared = LOOK_DIRECTION_EPSILON * LOOK_DIRECTION_EPSILON;
    float norm = sqrt(chroma_squared + epsilon_squared);
    float cosine = (a * skin.x + b * skin.y) / norm;
    float unit = clamp((cosine - skin.z) / (1.0 - skin.z), 0.0, 1.0);
    return unit * unit * (3.0 - 2.0 * unit);
}

// Эффект полос в точке круга `hue` ∈ [0, 2π): x — сдвиг оттенка (радианы), y — множитель цветности,
// z — множитель светлоты (LookBands::effect_at: приподнятый косинус между соседними центрами).
vec3 look_band_effect(float hue, uint count) {
    // Последний центр не правее оттенка; левее первого центра — последняя полоса (через круг).
    uint lower = count - 1u;
    for (uint index = 0u; index < count; ++index) {
        if (look_band_value(LOOK_ROW_BAND_CENTERS, index) <= hue) {
            lower = index;
        }
    }
    uint upper = (lower + 1u) % count;
    float lower_center = look_band_value(LOOK_ROW_BAND_CENTERS, lower);
    float span = look_band_value(LOOK_ROW_BAND_CENTERS, upper) - lower_center;
    if (span <= 0.0) {
        span += LOOK_TAU;
    }
    float offset = hue - lower_center;
    if (offset < 0.0) {
        offset += LOOK_TAU;
    }
    float upper_weight = 0.5 * (1.0 - cos(LOOK_PI * (offset / span)));
    float lower_weight = 1.0 - upper_weight;
    return vec3(
        look_band_value(LOOK_ROW_BAND_HUE_SHIFT, lower) * lower_weight +
            look_band_value(LOOK_ROW_BAND_HUE_SHIFT, upper) * upper_weight,
        look_band_value(LOOK_ROW_BAND_CHROMA, lower) * lower_weight +
            look_band_value(LOOK_ROW_BAND_CHROMA, upper) * upper_weight,
        look_band_value(LOOK_ROW_BAND_LIGHTNESS, lower) * lower_weight +
            look_band_value(LOOK_ROW_BAND_LIGHTNESS, upper) * upper_weight);
}

// Стадия цвета над светом W одного пикселя (LookColorParams::apply). Нейтральный вид (флаг 0) — вход как есть,
// бит в бит.
vec3 look_apply_color(vec3 working_rgb) {
    if (look.rows[LOOK_ROW_FLAGS].x < 0.5) {
        return working_rgb;
    }
    vec4 chroma_numbers = look.rows[LOOK_ROW_CHROMA];
    OklabMatrices oklab = look_oklab();
    vec3 lab = oklab_to_lab(oklab, working_rgb);
    // Колёса по зонам — первый шаг после перевода в OKLab (как в `LookColorParams::apply`); нейтральные пропущены.
    if (look.rows[LOOK_ROW_WHEELS].x >= 0.5) {
        lab = look_apply_wheels(lab);
    }
    float lightness = lab.x;
    float a = lab.y;
    float b = lab.z;
    float chroma_squared = a * a + b * b;

    float reference_squared = chroma_numbers.z * chroma_numbers.z;
    float skin = look_skin_window(a, b, chroma_squared);
    float vibrance_weight =
        (1.0 - look.rows[LOOK_ROW_SKIN].w * skin) / (1.0 + chroma_squared / reference_squared);
    float chroma_multiplier = chroma_numbers.x * (1.0 + chroma_numbers.y * vibrance_weight);

    float rotated_a = a;
    float rotated_b = b;
    float lightness_multiplier = 1.0;
    uint band_count = uint(look.rows[LOOK_ROW_FLAGS].y + 0.5);
    if (band_count > 0u) {
        float floor_squared = chroma_numbers.w * chroma_numbers.w;
        float not_grey = chroma_squared / (chroma_squared + floor_squared);
        // atan(0, 0) в GLSL не определён: при точно нулевой цветности оттенок любой — вес `not_grey` и так 0.
        float hue = chroma_squared > 0.0 ? atan(b, a) : 0.0;
        vec3 effect = look_band_effect(hue < 0.0 ? hue + LOOK_TAU : hue, band_count);
        float angle = not_grey * effect.x;
        float sine = sin(angle);
        float cosine = cos(angle);
        rotated_a = a * cosine - b * sine;
        rotated_b = a * sine + b * cosine;
        chroma_multiplier *= 1.0 + not_grey * (effect.y - 1.0);
        lightness_multiplier = 1.0 + not_grey * (effect.z - 1.0);
    }
    chroma_multiplier = max(chroma_multiplier, 0.0);
    return oklab_to_working(
        oklab, vec3(lightness * lightness_multiplier, rotated_a * chroma_multiplier, rotated_b * chroma_multiplier));
}

// ----- Таблица (LUT) ---------------------------------------------------------------------------------------

// Таблица подключена (в заголовке буфера размер > 0).
bool look_lut_present() {
    return look_lut.header.x != 0u;
}

// Узел таблицы `(red, green, blue)`: раскладка `r + N·(g + N·b)`, по три числа на узел.
vec3 look_lut_node(uvec3 corner, uint size) {
    uint base = 3u * (corner.x + size * (corner.y + size * corner.z));
    return vec3(look_lut.values[base], look_lut.values[base + 1u], look_lut.values[base + 2u]);
}

// Значение таблицы при кодированном входе `encoded` ∈ [0, 1]³: тетраэдральная интерполяция
// (LookLut::sample_encoded). Куб [i, i+1]³ делится на 6 тетраэдров по порядку дробных частей; путь от узла V000
// к V111 добавляет оси по убыванию дробной части (равные — в порядке R, G, B).
vec3 look_lut_sample_encoded(vec3 encoded) {
    uint size = look_lut.header.x;
    float last = float(size - 1u);
    uint max_cell = size - 2u;
    uvec3 cell;
    vec3 fraction;
    for (uint axis = 0u; axis < 3u; ++axis) {
        float position = clamp(encoded[axis], 0.0, 1.0) * last;
        cell[axis] = min(uint(floor(position)), max_cell);
        fraction[axis] = position - float(cell[axis]);
    }
    // Устойчивая сортировка осей по убыванию дробной части: строгое `>` не переставляет равные.
    uvec3 order = uvec3(0u, 1u, 2u);
    if (fraction[order.y] > fraction[order.x]) { order.xy = order.yx; }
    if (fraction[order.z] > fraction[order.y]) { order.yz = order.zy; }
    if (fraction[order.y] > fraction[order.x]) { order.xy = order.yx; }

    uvec3 corner = cell;
    vec3 previous = look_lut_node(corner, size);
    vec3 result = previous;
    for (uint step = 0u; step < 3u; ++step) {
        uint axis = order[step];
        corner[axis] += 1u;
        vec3 next = look_lut_node(corner, size);
        result += fraction[axis] * (next - previous);
        previous = next;
    }
    return result;
}

// Таблица над светом W (LookLut::apply): кодирование sRGB OETF (NaN → 0, обрезка 0…1) → таблица → обратно в
// линейный свет. Таблицы нет — вход как есть.
vec3 look_apply_lut(vec3 working_rgb) {
    if (!look_lut_present()) {
        return working_rgb;
    }
    vec3 encoded = color_tail_encode_srgb(working_rgb);
    vec3 sampled = look_lut_sample_encoded(encoded);
    return vec3(
        color_tail_srgb_decode(sampled.x), color_tail_srgb_decode(sampled.y), color_tail_srgb_decode(sampled.z));
}

#endif
