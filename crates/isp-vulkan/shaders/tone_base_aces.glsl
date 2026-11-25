// Основа тона ACES на GPU (решения 176, 177, 182): RRT и выходное преобразование SDR 100 нит (ACES 1.x).
// ОДНА реализация для всех путей: слитый проход видоискателя и выходной проход качественного конвейера и снимка.
// Эталон — isp-model `tone/aces.rs` (AcesBase::map_scene_pixel), `aces_sweeteners.rs`, `aces_spline.rs`,
// `aces_tonescale.rs`; порядок формул на пиксель — в доке `tone/aces.rs`.
//
// Происхождение и лицензия. Структура преобразования, числовые константы (свечение, красный модификатор,
// десатурации, поправка окружения, белый и чёрный нормировки) и коэффициенты сегментированных сплайнов C5 (RRT) и
// C9 (ODT 48 нит) взяты из Academy Color Encoding System (ampas/aces-dev v1.3, https://github.com/ampas/aces-dev,
// файлы RRT.ctl, ACESlib.RRT_Common.ctl, ACESlib.Tonescales.ctl, ACESlib.Utilities_Color.ctl, ACESlib.ODT_Common.ctl,
// ODT.Academy.Rec2020_100nits_dim.ctl); код переписан на GLSL заново, чужой код не копировался.
//
// (Дословно, ampas/aces-dev LICENSE.md.)
// License Terms for Academy Color Encoding System Components
//
// Academy Color Encoding System (ACES) software and tools are provided by the Academy under the following terms
// and conditions: A worldwide, royalty-free, non-exclusive right to copy, modify, create derivatives, and use, in
// source and binary forms, is hereby granted, subject to acceptance of this license.
//
// Copyright © 2015 Academy of Motion Picture Arts and Sciences (A.M.P.A.S.). Portions contributed by others as
// indicated. All rights reserved.
//
// Performance of any of the aforementioned acts indicates acceptance to be bound by the following terms and
// conditions:
//
// * Copies of source code, in whole or in part, must retain the above copyright notice, this list of conditions
//   and the Disclaimer of Warranty.
// * Use in binary form must retain the above copyright notice, this list of conditions and the Disclaimer of
//   Warranty in the documentation and/or other materials provided with the distribution.
// * Nothing in this license shall be deemed to grant any rights to trademarks, copyrights, patents, trade secrets
//   or any other intellectual property of A.M.P.A.S. or any contributors, except as expressly stated herein.
// * Neither the name "A.M.P.A.S." nor the name of any other contributors to this software may be used to endorse or
//   promote products derivative of or based on this software without express prior written permission of
//   A.M.P.A.S. or the contributors, as appropriate.
//
// This license shall be construed pursuant to the laws of the State of California, and any disputes related
// thereto shall be subject to the jurisdiction of the courts therein.
//
// Disclaimer of Warranty: THIS SOFTWARE IS PROVIDED BY A.M.P.A.S. AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
// IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE, AND NON-INFRINGEMENT ARE DISCLAIMED. IN NO EVENT SHALL A.M.P.A.S., OR ANY CONTRIBUTORS OR
// DISTRIBUTORS, BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, RESITUTIONARY, OR
// CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
// STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//
// WITHOUT LIMITING THE GENERALITY OF THE FOREGOING, THE ACADEMY SPECIFICALLY DISCLAIMS ANY REPRESENTATIONS OR
// WARRANTIES WHATSOEVER RELATED TO PATENT OR OTHER INTELLECTUAL PROPERTY RIGHTS IN THE ACADEMY COLOR ENCODING
// SYSTEM, OR APPLICATIONS THEREOF, HELD BY PARTIES OTHER THAN A.M.P.A.S., WHETHER DISCLOSED OR UNDISCLOSED.
// (Полный текст лицензии и условия для владельца приложения — в доке `crates/isp-model/src/tone/aces.rs`.)
//
// Порядок на пиксель (рабочее пространство W — линейный Rec. 2020):
//   вход (обрезка нулём | мягкое сжатие охвата W) → ×0,18/grey_eff, зажим 0…65504 → W→AP0 → свечение и красный
//   модификатор → зажим → AP0→AP1 → зажим → десатурация RRT → по каналам: форма осей, кривая (сплайны RRT и ODT в
//   log10, колено, белый точки) → поправка окружения Y^(γ_s−1) → AP1→W (с десатурацией выхода).
//
// Раскладка чисел модели в общем блоке `ToneBaseParameters` (`tone_base.glsl`); строки считаются от
// `ACES_FIRST_ROW` (строка 0 блока — общая). Зеркало `src/tone_base/aces.rs` (менять только вместе):
//   +0..+2   матрица XYZ D50 → W            +3..+5   матрица W → цвета выхода
//   +6..+8   матрица W → AP0                +9..+11  матрица AP0 → AP1        +12..+14 матрица AP1 → W
//   +15      xyz — строка яркости AP1, w — множитель входа 0,18/grey_eff
//   +16      x — сила свечения, y — масштаб красного модификатора, z — десатурация RRT, w — γ_s − 1
//   +17      флаги: x — вход (0.0 обрезка нулём, 1.0 мягкое сжатие)
//   +18      вход: xyz — порог сжатия по каналам, w — степень p        +19  вход: xyz — масштаб, w — 1/p
//   +20      выход: xyz — порог, w — степень                           +21  выход: xyz — масштаб, w — 1/p
//   +22      x — серый сцены grey_eff, y — контраст, z — сдвиг теней, w — сдвиг светов
//   +23      x — ширина перехода, y — log10(0,18), z — сдвиг экспозиции δ, w — колено k
//   +24      x — значение в колене, y — наклон в колене, z — предел белого E_cap, w — чёрный Y_min
//   +25      x — 1/(Y_max − Y_min), y — log10(Y_min)
//   +26..+34 сплайн RRT (C5)                 +35..+43 сплайн ODT (C9)
//   сплайн (9 строк): +0..+2 коэффициенты нижнего участка (10 в 12 ячейках), +3..+5 верхнего,
//     +6 log10 точек min, mid, max и (N−1)/(log_mid − log_min), +7 (N−1)/(log_max − log_mid), номер последнего
//     участка, наклон нижней и верхней прямых, +8 свободные члены нижней и верхней прямых.
// Всего 44 строки модели.
//
// Правила файла:
//   * Файл ничего не знает о выключателях стадий видоискателя: включающий файл сам решает, вызывать ли шаги.
//   * Всё, что на CPU считается отдельными умножением и сложением, объявлено `precise` (запрет FMA): ошибки
//     в ULP входят в тон ступенями, а сплайн и десатурации — цепочки из многих операций.
//   * Оттенок красного модификатора — тот же многочлен арктангенса, что у эталона (`atan2` шейдера не используем:
//     его точность допускает тысячи ULP).
//   * Включать только один раз на шейдер.
#ifndef TONE_BASE_ACES_GLSL
#define TONE_BASE_ACES_GLSL

#include "tone_base.glsl"

const uint ACES_FIRST_ROW = TONE_BASE_FIRST_MODEL_ROW;
const uint ACES_ROW_XYZ_D50_TO_WORKING = ACES_FIRST_ROW + 0u;
const uint ACES_ROW_WORKING_TO_OUTPUT = ACES_FIRST_ROW + 3u;
const uint ACES_ROW_WORKING_TO_AP0 = ACES_FIRST_ROW + 6u;
const uint ACES_ROW_AP0_TO_AP1 = ACES_FIRST_ROW + 9u;
const uint ACES_ROW_AP1_TO_WORKING = ACES_FIRST_ROW + 12u;
const uint ACES_ROW_LUMINANCE_EXPOSURE = ACES_FIRST_ROW + 15u;
const uint ACES_ROW_SWEETENERS = ACES_FIRST_ROW + 16u;
const uint ACES_ROW_FLAGS = ACES_FIRST_ROW + 17u;
const uint ACES_ROW_INPUT_THRESHOLD_POWER = ACES_FIRST_ROW + 18u;
const uint ACES_ROW_INPUT_SCALE_INVERSE_POWER = ACES_FIRST_ROW + 19u;
const uint ACES_ROW_OUTPUT_THRESHOLD_POWER = ACES_FIRST_ROW + 20u;
const uint ACES_ROW_OUTPUT_SCALE_INVERSE_POWER = ACES_FIRST_ROW + 21u;
const uint ACES_ROW_TONE_GREY = ACES_FIRST_ROW + 22u;
const uint ACES_ROW_TONE_SHAPE_SHIFT = ACES_FIRST_ROW + 23u;
const uint ACES_ROW_TONE_KNEE = ACES_FIRST_ROW + 24u;
const uint ACES_ROW_TONE_RANGE = ACES_FIRST_ROW + 25u;
const uint ACES_ROW_SPLINE_RRT = ACES_FIRST_ROW + 26u;
const uint ACES_ROW_SPLINE_ODT = ACES_FIRST_ROW + 35u;

// Строки внутри сплайна (смещение от его первой строки).
const uint ACES_SPLINE_COEFFICIENTS_LOW = 0u;
const uint ACES_SPLINE_COEFFICIENTS_HIGH = 3u;
const uint ACES_SPLINE_POINTS = 6u;
const uint ACES_SPLINE_SLOPES = 7u;
const uint ACES_SPLINE_INTERCEPTS = 8u;

// Константы эталона (`aces.rs`, `aces_spline.rs`, `aces_tonescale.rs`, `aces_sweeteners.rs`).
const float ACES_HALF_MIN = 6.103515625e-5;           // 2^-14: нижний вход сплайна (HALF_MIN в CTL)
const float ACES_HALF_MAX = 65504.0;                  // верхний зажим цвета в RRT
const float ACES_REFERENCE_GREY = 0.18;               // серая карта ACES
const float ACES_MIN_WHITE_LEVEL = 1.0;               // белый точки не ниже 1 (isp_model::tone::agx::MIN_WHITE_LEVEL)
const float ACES_LOG10_2 = 0.301029995663981;         // std::f32::consts::LOG10_2
const float ACES_LOG2_10 = 3.321928094887362;         // std::f32::consts::LOG2_10
const float ACES_SURROUND_BLACK_LUMINANCE = 6.0e-11;  // ниже — чёрный, поправка окружения не применяется
const float ACES_GLOW_MIDPOINT = 0.08;                // RRT_GLOW_MID
const float ACES_RED_PIVOT = 0.03;                    // RRT_RED_PIVOT
const float ACES_RED_WIDTH_DEGREES = 135.0;           // RRT_RED_WIDTH
const float ACES_SATURATION_FLOOR = 1e-10;            // TINY
const float ACES_SATURATION_DENOMINATOR_FLOOR = 1e-2;
const float ACES_YC_RADIUS_WEIGHT = 1.75;
const float ACES_RADIANS_TO_DEGREES = 57.29578;       // 180 / π, как `180.0 / PI` в f32
const float ACES_SQRT_3 = 1.7320508;                  // 3.0_f32.sqrt()
const float ACES_HALF_PI = 1.5707964;                 // FRAC_PI_2
const float ACES_PI = 3.1415927;

// Матрица модели с первой строки `first_row` блока · пиксель (общая функция `tone_base.glsl`).
vec3 aces_apply_matrix(uint first_row, vec3 pixel) {
    return tone_base_apply_matrix(first_row, pixel);
}

// Вход основы: NaN в любом канале — чёрный пиксель (как у эталона); затем обрезка нулём или мягкое сжатие
// охвата W (WorkingSpaceIo::apply_input).
vec3 aces_apply_input(vec3 scene) {
    if (color_tail_has_nan(scene)) {
        return vec3(0.0);
    }
    if (tone_base.rows[ACES_ROW_FLAGS].x > 0.5) {
        vec4 threshold_power = tone_base.rows[ACES_ROW_INPUT_THRESHOLD_POWER];
        vec4 scale_inverse_power = tone_base.rows[ACES_ROW_INPUT_SCALE_INVERSE_POWER];
        return color_tail_compress_gamut(
            scene, threshold_power.xyz, scale_inverse_power.xyz, threshold_power.w, scale_inverse_power.w);
    }
    return max(scene, vec3(0.0));
}

// ----- «Подслащивание» RRT: свечение и красный модификатор (aces_sweeteners.rs) -------------------------

// Насыщенность по ACES (rgb_2_saturation).
float aces_saturation(vec3 rgb) {
    float largest = max(max(rgb.r, rgb.g), rgb.b);
    float smallest = min(min(rgb.r, rgb.g), rgb.b);
    precise float numerator = max(largest, ACES_SATURATION_FLOOR) - max(smallest, ACES_SATURATION_FLOOR);
    return numerator / max(largest, ACES_SATURATION_DENOMINATOR_FLOOR);
}

// Яркость-прокси YC (rgb_2_yc): подкоренное математически ≥ 0, округление может дать −0 → защита max.
float aces_luma_chroma_proxy(vec3 rgb) {
    precise float radicand = (rgb.b * (rgb.b - rgb.g) + rgb.g * (rgb.g - rgb.r)) + rgb.r * (rgb.r - rgb.b);
    float chroma = sqrt(max(radicand, 0.0));
    precise float sum = ((rgb.b + rgb.g) + rgb.r) + ACES_YC_RADIUS_WEIGHT * chroma;
    return sum / 3.0;
}

// Гладкий переход 0…1 на x ∈ [−2, 2] (sigmoid_shaper).
float aces_sigmoid_shaper(float x) {
    float t = max(1.0 - abs(x / 2.0), 0.0);
    return (1.0 + sign(x) * (1.0 - t * t)) / 2.0;
}

// Добавка свечения (glow_fwd) при входном усилении `gain`.
float aces_glow_addition(float luma_chroma, float gain) {
    if (luma_chroma <= 2.0 / 3.0 * ACES_GLOW_MIDPOINT) {
        return gain;
    }
    if (luma_chroma >= 2.0 * ACES_GLOW_MIDPOINT) {
        return 0.0;
    }
    return gain * (ACES_GLOW_MIDPOINT / luma_chroma - 0.5);
}

// Арктангенс на [0, 1]: Абрамовиц и Стиген, формула 4.4.49 (ошибка ≤ 2·10⁻⁸ рад); схема Горнера по x².
float aces_atan_unit_interval(float x) {
    precise float x2 = x * x;
    precise float series = 0.0028662257;
    series = series * x2 - 0.016165737;
    series = series * x2 + 0.042909614;
    series = series * x2 - 0.07528964;
    series = series * x2 + 0.10656264;
    series = series * x2 - 0.142089;
    series = series * x2 + 0.19993551;
    series = series * x2 - 0.33333145;
    precise float result = (series * x2 + 1.0) * x;
    return result;
}

// atan2(y, x) на многочлене, радианы (−π, π]; (0, 0) даёт 0 (atan2_polynomial).
float aces_atan2_polynomial(float y, float x) {
    float abs_x = abs(x);
    float abs_y = abs(y);
    float largest = max(abs_x, abs_y);
    if (largest <= 0.0) {
        return 0.0;
    }
    float angle = aces_atan_unit_interval(min(abs_x, abs_y) / largest);
    if (abs_y > abs_x) {
        angle = ACES_HALF_PI - angle;
    }
    if (x < 0.0) {
        angle = ACES_PI - angle;
    }
    if (y < 0.0) {
        angle = -angle;
    }
    return angle;
}

// Вес кубического B-сплайна шириной `width` градусов вокруг нуля (cubic_basis_shaper).
float aces_cubic_basis_shaper(float hue_degrees, float width) {
    float half_width = width / 2.0;
    if (!(hue_degrees > -half_width && hue_degrees < half_width)) {
        return 0.0;
    }
    float knot_coordinate = (hue_degrees + half_width) * 4.0 / width;
    uint segment = uint(knot_coordinate);
    float t = knot_coordinate - float(segment);
    float value;
    if (segment == 0u) {
        value = t * t * t / 6.0;
    } else if (segment == 1u) {
        value = (-3.0 * t * t * t + 3.0 * t * t + 3.0 * t + 1.0) / 6.0;
    } else if (segment == 2u) {
        value = (3.0 * t * t * t - 6.0 * t * t + 4.0) / 6.0;
    } else {
        value = (-t * t * t + 3.0 * t * t - 3.0 * t + 1.0) / 6.0;
    }
    return value * 1.5;
}

// Вес красного модификатора по оттенку; у серого (все каналы равны) оттенка нет — вес 0.
float aces_red_hue_weight(vec3 rgb) {
    if (rgb.r == rgb.g && rgb.g == rgb.b) {
        return 0.0;
    }
    float hue_degrees =
        aces_atan2_polynomial(ACES_SQRT_3 * (rgb.g - rgb.b), 2.0 * rgb.r - rgb.g - rgb.b) * ACES_RADIANS_TO_DEGREES;
    return aces_cubic_basis_shaper(hue_degrees, ACES_RED_WIDTH_DEGREES);
}

// Свечение и красный модификатор для пикселя в AP0 (apply_sweeteners). Результат ещё не зажат.
vec3 aces_apply_sweeteners(vec3 aces) {
    vec4 sweeteners = tone_base.rows[ACES_ROW_SWEETENERS];
    float saturation = aces_saturation(aces);
    float luma_chroma = aces_luma_chroma_proxy(aces);
    float sigmoid = aces_sigmoid_shaper((saturation - 0.4) / 0.2);
    float glow = 1.0 + aces_glow_addition(luma_chroma, sweeteners.x * sigmoid);
    vec3 glowing = vec3(glow * aces.r, glow * aces.g, glow * aces.b);
    // Красный модификатор: оттенок уже после свечения, насыщенность — до него (как в RRT.ctl).
    float weight = aces_red_hue_weight(glowing);
    precise float shift = ((weight * saturation) * (ACES_RED_PIVOT - glowing.r)) * (1.0 - sweeteners.y);
    precise float red = glowing.r + shift;
    return vec3(red, glowing.g, glowing.b);
}

// ----- Тональная кривая по каналу (aces_spline.rs, aces_tonescale.rs) -----------------------------------

// Коэффициент `index` сплайна с первой строкой коэффициентов `first_row` (по четыре в строке).
float aces_spline_coefficient(uint first_row, uint index) {
    return tone_base.rows[first_row + index / 4u][index % 4u];
}

// Участок квадратичного B-сплайна: `knot_coordinate = (N − 1)·доля пути по участку`.
float aces_spline_segment(uint first_row, float knot_coordinate, uint last_segment) {
    uint index = min(uint(knot_coordinate), last_segment);
    float t = knot_coordinate - float(index);
    float c0 = aces_spline_coefficient(first_row, index);
    float c1 = aces_spline_coefficient(first_row, index + 1u);
    float c2 = aces_spline_coefficient(first_row, index + 2u);
    precise float quadratic = t * t * (0.5 * c0 - c1 + 0.5 * c2);
    precise float linear = t * (c1 - c0);
    precise float value = (quadratic + linear) + 0.5 * (c0 + c1);
    return value;
}

// log10 x → log10 y сегментированного сплайна, начинающегося с строки `first_row` блока.
float aces_spline_forward_log10(uint first_row, float log_x) {
    vec4 points = tone_base.rows[first_row + ACES_SPLINE_POINTS];
    vec4 slopes = tone_base.rows[first_row + ACES_SPLINE_SLOPES];
    vec4 intercepts = tone_base.rows[first_row + ACES_SPLINE_INTERCEPTS];
    uint last_segment = uint(slopes.y + 0.5);
    if (log_x <= points.x) {
        precise float below = log_x * slopes.z + intercepts.x;
        return below;
    }
    if (log_x < points.y) {
        return aces_spline_segment(first_row + ACES_SPLINE_COEFFICIENTS_LOW, (log_x - points.x) * points.w, last_segment);
    }
    if (log_x < points.z) {
        return aces_spline_segment(first_row + ACES_SPLINE_COEFFICIENTS_HIGH, (log_x - points.y) * slopes.x, last_segment);
    }
    precise float above = log_x * slopes.w + intercepts.y;
    return above;
}

// Форма осей (контраст, тени и света из настроек модели): e → e₁ (ToneScaleParameters::shape).
float aces_shape(float e) {
    vec4 grey = tone_base.rows[ACES_ROW_TONE_GREY];
    vec4 shape_shift = tone_base.rows[ACES_ROW_TONE_SHAPE_SHIFT];
    float squared = e * e;
    float sweep = e < 0.0 ? grey.z : grey.w;
    precise float width_squared = shape_shift.x * shape_shift.x;
    precise float bend = sweep * (squared / (squared + width_squared));
    precise float shaped = grey.y * e + bend;
    return shaped;
}

// Белый точки в ступенях E_L для уровня белого `white` (ToneScaleParameters::white_ev).
float aces_white_ev(float white) {
    float grey = tone_base.rows[ACES_ROW_TONE_GREY].x;
    float exposure = log2(max(white, ACES_MIN_WHITE_LEVEL) / grey);
    return min(aces_shape(exposure), tone_base.rows[ACES_ROW_TONE_KNEE].z);
}

// Эрмитова кубика от колена (значение, наклон) до белого точки (1, наклон 0).
float aces_approach_to_white(float exposure, float white_ev) {
    float knee = tone_base.rows[ACES_ROW_TONE_SHAPE_SHIFT].w;
    float knee_value = tone_base.rows[ACES_ROW_TONE_KNEE].x;
    float knee_slope = tone_base.rows[ACES_ROW_TONE_KNEE].y;
    float width = white_ev - knee;
    float t = (exposure - knee) / width;
    float t2 = t * t;
    float t3 = t2 * t;
    float start_value = 2.0 * t3 - 3.0 * t2 + 1.0;
    float start_slope = t3 - 2.0 * t2 + t;
    float end_value = -2.0 * t3 + 3.0 * t2;
    precise float result = (start_value * knee_value + start_slope * width * knee_slope) + end_value;
    return result;
}

// linCV одного канала AP1 при белом точки `white_ev` (ToneScaleParameters::linear_cv).
float aces_linear_cv(float value, float white_ev) {
    vec4 shape_shift = tone_base.rows[ACES_ROW_TONE_SHAPE_SHIFT];
    vec4 knee = tone_base.rows[ACES_ROW_TONE_KNEE];
    vec4 range = tone_base.rows[ACES_ROW_TONE_RANGE];
    float exposure = aces_shape(log2(max(value, ACES_HALF_MIN) / ACES_REFERENCE_GREY));
    if (exposure >= white_ev) {
        return 1.0;
    }
    if (exposure >= shape_shift.w) {
        return aces_approach_to_white(exposure, white_ev);
    }
    precise float log_x = shape_shift.y + (exposure + shape_shift.z) * ACES_LOG10_2;
    float log_y = aces_spline_forward_log10(ACES_ROW_SPLINE_ODT, aces_spline_forward_log10(ACES_ROW_SPLINE_RRT, log_x));
    if (log_y <= range.y) {
        return 0.0;
    }
    float nits = exp2(log_y * ACES_LOG2_10);
    precise float normalized = (nits - knee.w) * range.x;
    return clamp(normalized, 0.0, 1.0);
}

// Яркость AP1: строка яркости · цвет (AcesBase::luminance_of).
float aces_luminance(vec3 ap1) {
    return tone_base_matrix_row(tone_base.rows[ACES_ROW_LUMINANCE_EXPOSURE], ap1);
}

// Глобальная десатурация RRT: s·v + (1 − s)·Y(v) по каналам.
vec3 aces_desaturate_rrt(vec3 ap1) {
    float saturation = tone_base.rows[ACES_ROW_SWEETENERS].z;
    float luminance = aces_luminance(ap1);
    precise float gray_share = (1.0 - saturation) * luminance;
    precise float red = saturation * ap1.r + gray_share;
    precise float green = saturation * ap1.g + gray_share;
    precise float blue = saturation * ap1.b + gray_share;
    return vec3(red, green, blue);
}

// Поправка тусклого окружения: cv · Y^(γ_s − 1); чёрный не трогаем (степень от нуля бесконечна).
vec3 aces_apply_surround(vec3 linear_cv) {
    float luminance = aces_luminance(linear_cv);
    if (luminance <= ACES_SURROUND_BLACK_LUMINANCE) {
        return linear_cv;
    }
    float factor = pow(luminance, tone_base.rows[ACES_ROW_SWEETENERS].w);
    return linear_cv * factor;
}

// Основа ACES после входа (AcesBase::map_scene_pixel без входа). `entering` — результат aces_apply_input;
// `white` — свой белый точки L (решение 177). Результат — свет экрана в W.
vec3 aces_apply_base(vec3 entering, float white) {
    float exposure_scale = tone_base.rows[ACES_ROW_LUMINANCE_EXPOSURE].w;
    vec3 aces = clamp(entering * exposure_scale, vec3(0.0), vec3(ACES_HALF_MAX));
    vec3 ap0 = aces_apply_matrix(ACES_ROW_WORKING_TO_AP0, aces);
    vec3 sweetened = aces_apply_sweeteners(ap0);
    // RRT: зажим, чтобы насыщенные отрицательные не стали положительными после матрицы.
    vec3 ap1 = clamp(
        aces_apply_matrix(ACES_ROW_AP0_TO_AP1, clamp(sweetened, vec3(0.0), vec3(ACES_HALF_MAX))),
        vec3(0.0),
        vec3(ACES_HALF_MAX));
    vec3 desaturated = aces_desaturate_rrt(ap1);
    float white_ev = aces_white_ev(white);
    vec3 linear_cv = vec3(
        aces_linear_cv(desaturated.r, white_ev),
        aces_linear_cv(desaturated.g, white_ev),
        aces_linear_cv(desaturated.b, white_ev));
    return aces_apply_matrix(ACES_ROW_AP1_TO_WORKING, aces_apply_surround(linear_cv));
}

// Выход, шаг 1: W → цвета выхода (матрица).
vec3 aces_working_to_output(vec3 display_working) {
    return aces_apply_matrix(ACES_ROW_WORKING_TO_OUTPUT, display_working);
}

// Выход, шаг 2: мягкое сжатие охвата цветов выхода.
vec3 aces_compress_output_gamut(vec3 output_color) {
    vec4 threshold_power = tone_base.rows[ACES_ROW_OUTPUT_THRESHOLD_POWER];
    vec4 scale_inverse_power = tone_base.rows[ACES_ROW_OUTPUT_SCALE_INVERSE_POWER];
    return color_tail_compress_gamut(
        output_color, threshold_power.xyz, scale_inverse_power.xyz, threshold_power.w, scale_inverse_power.w);
}

// Выход, шаг 3: обрезка 0…1 (WorkingSpaceIo::to_output).
vec3 aces_clamp_output(vec3 output_color) {
    return clamp(output_color, vec3(0.0), vec3(1.0));
}

#endif
