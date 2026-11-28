// Полоски Lift / Gamma / Gain / Offset по каналам R, G, B на GPU (решения 339–343). Эталон — isp-model
// `look/primaries.rs` (`LookPrimaries::{apply, curve}`): формулы, вывод и условие монотонности там; здесь тот же
// порядок операций, без FMA (`precise`, как `look_wheels.glsl` и `look_tone.glsl`).
//
// Числа приходят готовыми строками блока `LookParameters` (зеркало `src/look/mod.rs`), первая строка —
// `LOOK_ROW_PRIMARIES`:
//   +0      — (полоски включены, c, √c, n)            константы кодирования с «носком»,
//   +1      — (1/n, 0, 0, 0),
//   +2…+4   — матрица W → основа каналов по строкам,
//   +5…+7   — матрица основа каналов → W по строкам,
//   +8      — lift  (l для R, G, B; 0),
//   +9      — gamma (наклон s для R, G, B; 1 — нейтраль),
//   +10     — gain  (G для R, G, B; 1 — нейтраль),
//   +11     — offset (o для R, G, B; 0).
//
// Включается только из `look_stage.glsl` (там объявлены `look` и `LOOK_ROW_PRIMARIES`); нейтральные полоски —
// флаг в строке `LOOK_ROW_PRIMARIES`, x (НЕ константа специализации: касание полоски не создаёт pipeline).
#ifndef LOOK_PRIMARIES_GLSL
#define LOOK_PRIMARIES_GLSL

// Кодирование с линейным «носком» (PrimariesEncoding::encode): нечётное, e(0) = 0, e(1) = 1.
float look_primaries_encode(float light, vec4 constants, float inverse_scale) {
    precise float magnitude = (sqrt(abs(light) + constants.y) - constants.z) * inverse_scale;
    return light < 0.0 ? -magnitude : magnitude;
}

// Обратное кодирование (PrimariesEncoding::decode): t·(t + 2√c), t = |e|·n.
float look_primaries_decode(float encoded, vec4 constants) {
    precise float stretched = abs(encoded) * constants.w;
    precise float magnitude = stretched * (stretched + 2.0 * constants.z);
    return encoded < 0.0 ? -magnitude : magnitude;
}

// Кривая одного канала (LookPrimaries::curve): кодирование → Gamma → Lift/Gain → Offset → декодирование.
float look_primaries_curve(float light, int channel, vec4 constants, float inverse_scale) {
    float lift = look.rows[LOOK_ROW_PRIMARIES + 8u][channel];
    float slope = look.rows[LOOK_ROW_PRIMARIES + 9u][channel];
    float gain = look.rows[LOOK_ROW_PRIMARIES + 10u][channel];
    float offset = look.rows[LOOK_ROW_PRIMARIES + 11u][channel];
    float encoded = look_primaries_encode(light, constants, inverse_scale);
    precise float gammed;
    if (encoded < 0.0) {
        gammed = slope * encoded;
    } else if (encoded > 1.0) {
        gammed = 1.0 + (encoded - 1.0) / slope;
    } else {
        precise float numerator = slope * encoded;
        precise float denominator = 1.0 + (slope - 1.0) * encoded;
        gammed = numerator / denominator;
    }
    precise float interpolated = lift * (1.0 - gammed) + gain * gammed;
    precise float shifted = interpolated + offset;
    return look_primaries_decode(shifted, constants);
}

// Полоски над светом W одного пикселя (LookPrimaries::apply). Нейтраль (флаг 0) — вход как есть, бит в бит.
vec3 look_apply_primaries(vec3 working_rgb) {
    vec4 constants = look.rows[LOOK_ROW_PRIMARIES];
    if (constants.x < 0.5) {
        return working_rgb;
    }
    float inverse_scale = look.rows[LOOK_ROW_PRIMARIES + 1u].x;
    vec3 basis_rgb = vec3(
        oklab_matrix_row(look.rows[LOOK_ROW_PRIMARIES + 2u], working_rgb),
        oklab_matrix_row(look.rows[LOOK_ROW_PRIMARIES + 3u], working_rgb),
        oklab_matrix_row(look.rows[LOOK_ROW_PRIMARIES + 4u], working_rgb));
    vec3 moved = vec3(
        look_primaries_curve(basis_rgb.x, 0, constants, inverse_scale),
        look_primaries_curve(basis_rgb.y, 1, constants, inverse_scale),
        look_primaries_curve(basis_rgb.z, 2, constants, inverse_scale));
    return vec3(
        oklab_matrix_row(look.rows[LOOK_ROW_PRIMARIES + 5u], moved),
        oklab_matrix_row(look.rows[LOOK_ROW_PRIMARIES + 6u], moved),
        oklab_matrix_row(look.rows[LOOK_ROW_PRIMARIES + 7u], moved));
}

#endif
