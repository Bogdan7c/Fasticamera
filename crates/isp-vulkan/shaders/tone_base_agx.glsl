// Основа тона AgX на GPU (решения 101, 176, 177). ОДНА реализация для всех путей: слитый проход видоискателя
// и выходной проход качественного конвейера и снимка. Эталон — isp-model `tone/agx.rs` (AgxBase::apply_input,
// map_scene_pixel, to_output), формулы кривой выведены в доке того модуля:
//   плечо (0 ≤ e < Δ_sh):  y = y_p + s·e / (1 + (e/Δ_sh)^p1 · (r1^p1 − 1))^(1/p1),  r1 = s·Δ_sh/(1 − y_p)
//   носок (−Δ_toe < e < 0): y = y_p − s·t / (1 + (t/Δ_toe)^p0 · (r0^p0 − 1))^(1/p0),  t = −e
// где e = log2(x / g) — ступени относительно серого g (с подтяжкой AE), Δ_sh = log2(max(L, 1) / g), L — свой
// белый точки (решение 177). Затем lin = y^γ, затем возврат оттенка (решение 180, nlerp без atan2/sin/cos;
// эталон — `tone/agx_hue.rs`), затем матрица «наружу».
//
// Раскладка чисел модели в общем блоке `ToneBaseParameters` (`tone_base.glsl`); строки считаются от
// `AGX_FIRST_ROW` (строка 0 блока — общая). Зеркало `src/tone_base/agx.rs` (менять только вместе):
//   +0..+2   матрица XYZ D50 → W            +3..+5   матрица «внутрь» (inset)
//   +6..+8   матрица «наружу» (outset)      +9..+11  матрица W → цвета выхода
//   +12      x — серый сцены с подтяжкой AE, y — y_p, z — γ, w — наклон s
//   +13      носок: x — p0, y — 1/p0, z — Δ_toe, w — r0^p0 − 1
//   +14      плечо: x — p1, y — 1/p1, z — s/(1 − y_p), w — 0
//   +15      вход: xyz — порог сжатия по каналам, w — степень p
//   +16      вход: xyz — масштаб, w — 1/p
//   +17      выход: xyz — порог, w — степень      +18  выход: xyz — масштаб, w — 1/p
//   +19      флаги: x — вход (0.0 обрезка нулём, 1.0 мягкое сжатие), y — степень возврата оттенка h
//            (0.0 — шаг пропущен), z — гладкий порог серого F возврата оттенка, w — 0
//   +20..+22 OKLab W → LMS     +23 строка яркости Y (xyz)     +24..+26 LMS' → Lab
//   +27..+29 Lab → LMS'        +30..+32 LMS → W
// Всего 33 строки модели.
//
// Правила файла:
//   * Файл ничего не знает о выключателях стадий видоискателя: включающий файл сам решает, вызывать ли шаги.
//   * Матрицы считаются без FMA слева направо (`precise`) — как PixelMatrix::apply.
//   * Включать только один раз на шейдер.
#ifndef TONE_BASE_AGX_GLSL
#define TONE_BASE_AGX_GLSL

#include "tone_base.glsl"
#include "oklab_working.glsl"

const uint AGX_FIRST_ROW = TONE_BASE_FIRST_MODEL_ROW;
const uint AGX_ROW_XYZ_D50_TO_WORKING = AGX_FIRST_ROW + 0u;
const uint AGX_ROW_INSET = AGX_FIRST_ROW + 3u;
const uint AGX_ROW_OUTSET = AGX_FIRST_ROW + 6u;
const uint AGX_ROW_WORKING_TO_OUTPUT = AGX_FIRST_ROW + 9u;
const uint AGX_ROW_CURVE_GREY = AGX_FIRST_ROW + 12u;
const uint AGX_ROW_CURVE_TOE = AGX_FIRST_ROW + 13u;
const uint AGX_ROW_CURVE_SHOULDER = AGX_FIRST_ROW + 14u;
const uint AGX_ROW_INPUT_THRESHOLD_POWER = AGX_FIRST_ROW + 15u;
const uint AGX_ROW_INPUT_SCALE_INVERSE_POWER = AGX_FIRST_ROW + 16u;
const uint AGX_ROW_OUTPUT_THRESHOLD_POWER = AGX_FIRST_ROW + 17u;
const uint AGX_ROW_OUTPUT_SCALE_INVERSE_POWER = AGX_FIRST_ROW + 18u;
const uint AGX_ROW_FLAGS = AGX_FIRST_ROW + 19u;
const uint AGX_ROW_HUE_OKLAB = AGX_FIRST_ROW + 20u;
const uint AGX_ROW_HUE_LUMINANCE = AGX_FIRST_ROW + 23u;

// Матрица модели с первой строки `first_row` блока · пиксель (общая функция `tone_base.glsl`).
vec3 agx_apply_matrix(uint first_row, vec3 pixel) {
    return tone_base_apply_matrix(first_row, pixel);
}

// Вход основы: NaN в любом канале — чёрный пиксель (как у эталона); затем обрезка нулём или мягкое сжатие
// охвата W (AgxInput::ClipToZero / SoftCompress).
vec3 agx_apply_input(vec3 scene) {
    if (color_tail_has_nan(scene)) {
        return vec3(0.0);
    }
    if (tone_base.rows[AGX_ROW_FLAGS].x > 0.5) {
        vec4 threshold_power = tone_base.rows[AGX_ROW_INPUT_THRESHOLD_POWER];
        vec4 scale_inverse_power = tone_base.rows[AGX_ROW_INPUT_SCALE_INVERSE_POWER];
        return color_tail_compress_gamut(
            scene, threshold_power.xyz, scale_inverse_power.xyz, threshold_power.w, scale_inverse_power.w);
    }
    return max(scene, vec3(0.0));
}

// Числа кривой, зависящие только от белого точки (одни на три канала пикселя).
struct AgxWhite {
    float shoulder_end;     // Δ_sh = log2(max(L, 1) / g): ступеней от серого до белого
    float shoulder_excess;  // r1^p1 − 1
};

AgxWhite agx_white(float white) {
    vec4 grey = tone_base.rows[AGX_ROW_CURVE_GREY];
    vec4 shoulder = tone_base.rows[AGX_ROW_CURVE_SHOULDER];
    float shoulder_end = log2(max(white, 1.0) / grey.x);
    float ratio = shoulder.z * shoulder_end;
    float excess = pow(ratio, shoulder.x) - 1.0;
    return AgxWhite(shoulder_end, excess);
}

// Кривая одного канала (AgxCurve::map_channel): выход y ∈ [0, 1] до y^γ. x ≤ 0 и NaN → 0.
float agx_curve_channel(float light, AgxWhite white) {
    if (isnan(light) || light <= 0.0) {
        return 0.0;
    }
    vec4 grey = tone_base.rows[AGX_ROW_CURVE_GREY];
    vec4 toe = tone_base.rows[AGX_ROW_CURVE_TOE];
    vec4 shoulder = tone_base.rows[AGX_ROW_CURVE_SHOULDER];
    float stops = log2(light / grey.x);
    if (stops >= white.shoulder_end) {
        return 1.0;
    }
    float gray_output = grey.y;
    float slope = grey.w;
    float curve;
    if (stops >= 0.0) {
        float blend = pow(stops / white.shoulder_end, shoulder.x) * white.shoulder_excess;
        curve = gray_output + slope * stops / pow(1.0 + blend, shoulder.y);
    } else {
        float depth = -stops;
        if (depth >= toe.z) {
            return 0.0;
        }
        float blend = pow(depth / toe.z, toe.x) * toe.w;
        curve = gray_output - slope * depth / pow(1.0 + blend, toe.y);
    }
    return clamp(curve, 0.0, 1.0);
}

// ----- Возврат оттенка (решение 180): эталон — isp-model `tone/agx_hue.rs` -------------------------------

// Малая добавка к знаменателю нормировки направления (для c → 0), не порог.
const float AGX_HUE_DIRECTION_EPSILON = 1e-6;
// Ниже этой длины смеси направлений («векторы противоположны») берём выходное направление.
const float AGX_HUE_DEGENERATE_BLEND_LENGTH = 1e-6;
// Ограничение приведения яркости: k ≤ 1000.
const float AGX_HUE_MAX_LUMINANCE_RATIO_INVERSE = 1e-3;
// Защита знаменателя от нуля (абсолютная малость).
const float AGX_HUE_TINY_LUMINANCE = 1e-30;

// Матрицы OKLab W (Rec. 2020) из строк модели.
OklabMatrices agx_oklab() {
    OklabMatrices matrices;
    for (uint row = 0u; row < 3u; ++row) {
        matrices.working_to_lms[row] = tone_base.rows[AGX_ROW_HUE_OKLAB + row];
        matrices.lms_prime_to_lab[row] = tone_base.rows[AGX_ROW_HUE_OKLAB + 4u + row];
        matrices.lab_to_lms_prime[row] = tone_base.rows[AGX_ROW_HUE_OKLAB + 7u + row];
        matrices.lms_to_working[row] = tone_base.rows[AGX_ROW_HUE_OKLAB + 10u + row];
    }
    return matrices;
}

// Возвращает оттенок `before` (до кривой, после inset) цвету `after` (после кривой и y^γ): светлота и
// цветность остаются выходными, направление оттенка — nlerp между выходным и входным (HueRestoration::restore).
vec3 agx_restore_hue(vec3 before, vec3 after) {
    float degree = tone_base.rows[AGX_ROW_FLAGS].y;
    float chroma_floor = tone_base.rows[AGX_ROW_FLAGS].z;
    vec4 luminance_row = tone_base.rows[AGX_ROW_HUE_LUMINANCE];
    float after_luminance = oklab_matrix_row(luminance_row, after);
    float before_luminance = oklab_matrix_row(luminance_row, before);
    float scale = after_luminance /
        max(before_luminance, AGX_HUE_MAX_LUMINANCE_RATIO_INVERSE * after_luminance + AGX_HUE_TINY_LUMINANCE);
    OklabMatrices oklab = agx_oklab();
    vec3 before_lab = oklab_to_lab(oklab, before * scale);
    vec3 after_lab = oklab_to_lab(oklab, after);

    float before_chroma_squared = before_lab.y * before_lab.y + before_lab.z * before_lab.z;
    float weight = degree * before_chroma_squared / (before_chroma_squared + chroma_floor * chroma_floor);

    float epsilon_squared = AGX_HUE_DIRECTION_EPSILON * AGX_HUE_DIRECTION_EPSILON;
    float before_norm = sqrt(before_chroma_squared + epsilon_squared);
    float after_chroma_squared = after_lab.y * after_lab.y + after_lab.z * after_lab.z;
    float after_norm = sqrt(after_chroma_squared + epsilon_squared);
    float blend_a = (1.0 - weight) * after_lab.y / after_norm + weight * before_lab.y / before_norm;
    float blend_b = (1.0 - weight) * after_lab.z / after_norm + weight * before_lab.z / before_norm;
    float blend_length = length(vec2(blend_a, blend_b));

    float new_a = after_lab.y;
    float new_b = after_lab.z;
    if (blend_length > AGX_HUE_DEGENERATE_BLEND_LENGTH) {
        float after_chroma = sqrt(after_chroma_squared);
        new_a = blend_a * after_chroma / blend_length;
        new_b = blend_b * after_chroma / blend_length;
    }
    return oklab_to_working(oklab, vec3(after_lab.x, new_a, new_b));
}

// Основа AgX после входа: inset → кривая по каналам → y^γ → возврат оттенка → outset
// (AgxBase::map_scene_pixel без входа). `entering` — результат agx_apply_input; `white` — свой белый точки L
// (решение 177).
vec3 agx_apply_base(vec3 entering, float white) {
    AgxWhite white_numbers = agx_white(white);
    vec3 inset = agx_apply_matrix(AGX_ROW_INSET, entering);
    float gamma = tone_base.rows[AGX_ROW_CURVE_GREY].z;
    vec3 shaped = vec3(
        pow(agx_curve_channel(inset.r, white_numbers), gamma),
        pow(agx_curve_channel(inset.g, white_numbers), gamma),
        pow(agx_curve_channel(inset.b, white_numbers), gamma));
    // Степень 0 пропускает шаг целиком (выход бит в бит прежнего AgX).
    if (tone_base.rows[AGX_ROW_FLAGS].y > 0.0) {
        shaped = agx_restore_hue(inset, shaped);
    }
    return agx_apply_matrix(AGX_ROW_OUTSET, shaped);
}

// Выход, шаг 1: W → цвета выхода (матрица).
vec3 agx_working_to_output(vec3 display_working) {
    return agx_apply_matrix(AGX_ROW_WORKING_TO_OUTPUT, display_working);
}

// Выход, шаг 2: мягкое сжатие охвата цветов выхода.
vec3 agx_compress_output_gamut(vec3 output_color) {
    vec4 threshold_power = tone_base.rows[AGX_ROW_OUTPUT_THRESHOLD_POWER];
    vec4 scale_inverse_power = tone_base.rows[AGX_ROW_OUTPUT_SCALE_INVERSE_POWER];
    return color_tail_compress_gamut(
        output_color, threshold_power.xyz, scale_inverse_power.xyz, threshold_power.w, scale_inverse_power.w);
}

// Выход, шаг 3: обрезка 0…1 (AgxBase::to_output).
vec3 agx_clamp_output(vec3 output_color) {
    return clamp(output_color, vec3(0.0), vec3(1.0));
}

#endif
