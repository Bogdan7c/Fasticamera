// Основа тона filmic на GPU (решения 176, 177, 182). ОДНА реализация для всех путей: слитый проход видоискателя
// и выходной проход качественного конвейера и снимка. Эталон — isp-model `tone/filmic.rs`
// (`FilmicBase::map_scene_pixel`, `apply_input`, `to_output`); формулы кривой выведены в доке того модуля. Порядок
// операций пикселя (зеркало эталона, `rebuild_from_parameters` в `filmic_knob_tests.rs`):
//   вход (NaN → чёрный; обрезка нулём или мягкое сжатие охвата W; затем clamp 0…MAX_SCENE_LIGHT)
//   → норма N (PerChannel / MaxRgb / Luminance / PowerNorm) → e = log2(N/g) → кривая y(e; Δ_w) → y^γ
//   → вес обесцвечивания d = smootherstep((y − o)/(1 − o)) → цвет display·(c/N) → смесь в OKLab к белому → «вписывание»
//   в охват выхода (делим на наибольший канал `W → выход`, если он выше 1 + FIT_TOLERANCE).
// У PerChannel нормы нет: out_c = y(c)^γ по каждому каналу, без обесцвечивания и вписывания.
// Дальше (вне этого файла) идёт слой вида, затем выходной шаг: W → цвета выхода, мягкое сжатие охвата, обрезка 0…1.
//
// Раскладка чисел модели в общем блоке `ToneBaseParameters` (`tone_base.glsl`); строки считаются от
// `FILMIC_FIRST_ROW` (строка 0 блока — общая). Зеркало `src/tone_base/filmic.rs` (менять только вместе):
//   +0..+2   матрица XYZ D50 → W            +3..+5   матрица W → цвета выхода
//   +6       x — серый сцены с подтяжкой AE g, y — y_p, z — γ, w — наклон s
//   +7       носок: x — a (длина линейного участка вниз, EV), y — y_lo, z — w_t, w — r0^p0 − 1
//   +8       степени: x — p0, y — 1/p0, z — p1, w — 1/p1
//   +9       плечо: x — b (длина линейного участка вверх, EV), y — y_hi, z — h = 1 − y_hi, w — порог обесцвечивания o
//   +10      вход: xyz — порог сжатия по каналам, w — степень p      +11  вход: xyz — масштаб, w — 1/p
//   +12      выход: xyz — порог, w — степень                         +13  выход: xyz — масштаб, w — 1/p
//   +14      флаги: x — вход (0.0 обрезка нулём, 1.0 мягкое сжатие), y — норма (FILMIC_NORM_*), z, w — 0
//   +15      строка яркости Y (xyz), w — 0
//   +16..+18 OKLab W → LMS    +19..+21 LMS' → Lab    +22..+24 Lab → LMS'    +25..+27 LMS → W
// Всего 28 строк модели.
//
// Правила файла:
//   * Файл ничего не знает о выключателях стадий видоискателя: включающий файл сам решает, вызывать ли шаги.
//   * Матрицы, норма, смесь OKLab и весовая функция считаются без FMA слева направо (`precise`) — как на CPU.
//   * Включать только один раз на шейдер (защита от повторного включения есть). Рядом можно включать
//     `tone_base_agx.glsl` и другие модели: имена здесь начинаются с `filmic_` / `FILMIC_`.
#ifndef TONE_BASE_FILMIC_GLSL
#define TONE_BASE_FILMIC_GLSL

#include "tone_base.glsl"
#include "oklab_working.glsl"

const uint FILMIC_FIRST_ROW = TONE_BASE_FIRST_MODEL_ROW;
const uint FILMIC_ROW_XYZ_D50_TO_WORKING = FILMIC_FIRST_ROW + 0u;
const uint FILMIC_ROW_WORKING_TO_OUTPUT = FILMIC_FIRST_ROW + 3u;
const uint FILMIC_ROW_CURVE_GREY = FILMIC_FIRST_ROW + 6u;
const uint FILMIC_ROW_CURVE_TOE = FILMIC_FIRST_ROW + 7u;
const uint FILMIC_ROW_CURVE_POWERS = FILMIC_FIRST_ROW + 8u;
const uint FILMIC_ROW_CURVE_SHOULDER = FILMIC_FIRST_ROW + 9u;
const uint FILMIC_ROW_INPUT_THRESHOLD_POWER = FILMIC_FIRST_ROW + 10u;
const uint FILMIC_ROW_INPUT_SCALE_INVERSE_POWER = FILMIC_FIRST_ROW + 11u;
const uint FILMIC_ROW_OUTPUT_THRESHOLD_POWER = FILMIC_FIRST_ROW + 12u;
const uint FILMIC_ROW_OUTPUT_SCALE_INVERSE_POWER = FILMIC_FIRST_ROW + 13u;
const uint FILMIC_ROW_FLAGS = FILMIC_FIRST_ROW + 14u;
const uint FILMIC_ROW_LUMINANCE = FILMIC_FIRST_ROW + 15u;
const uint FILMIC_ROW_OKLAB = FILMIC_FIRST_ROW + 16u;

// Коды нормы (зеркало `norm_code` в `src/tone_base/filmic.rs` и порядка `FilmicNorm`): в строке 14 лежат числом float.
const float FILMIC_NORM_PER_CHANNEL = 0.0;
const float FILMIC_NORM_MAX_RGB = 1.0;
const float FILMIC_NORM_LUMINANCE = 2.0;
const float FILMIC_NORM_POWER = 3.0;

// Свет сцены выше этого значения обрезается (зеркало MAX_SCENE_LIGHT эталона): защита от inf/inf в делении c/N и
// от переполнения куба в степенной норме.
const float FILMIC_MAX_SCENE_LIGHT = 1.0e20;
// Допуск «вписывания» в охват выхода (зеркало FIT_TOLERANCE эталона): белый и серый вписыванием не трогаются.
const float FILMIC_FIT_TOLERANCE = 1.0e-5;
// Наименьший белый точки (зеркало MIN_WHITE_LEVEL эталона).
const float FILMIC_MIN_WHITE_LEVEL = 1.0;

// Матрица модели с первой строки `first_row` блока · пиксель (общая функция `tone_base.glsl`).
vec3 filmic_apply_matrix(uint first_row, vec3 pixel) {
    return tone_base_apply_matrix(first_row, pixel);
}

// Вход основы: NaN в любом канале — чёрный пиксель (как у эталона); затем обрезка нулём или мягкое сжатие охвата W
// (общий шаг WorkingSpaceIo::apply_input — тот же, что у AgX).
vec3 filmic_apply_input(vec3 scene) {
    if (color_tail_has_nan(scene)) {
        return vec3(0.0);
    }
    if (tone_base.rows[FILMIC_ROW_FLAGS].x > 0.5) {
        vec4 threshold_power = tone_base.rows[FILMIC_ROW_INPUT_THRESHOLD_POWER];
        vec4 scale_inverse_power = tone_base.rows[FILMIC_ROW_INPUT_SCALE_INVERSE_POWER];
        return color_tail_compress_gamut(
            scene, threshold_power.xyz, scale_inverse_power.xyz, threshold_power.w, scale_inverse_power.w);
    }
    return max(scene, vec3(0.0));
}

// ----- Кривая (FilmicCurve::map_light) -----------------------------------------------------------------------

// Выход кривой y ∈ [0, 1] до y^γ для света `light` (нормы пикселя или канала) и ступеней до белого точки `delta`
// = log2(max(L, 1)/g) (одна на пиксель: её считает `filmic_white_stops`). x ≤ 0 и NaN → 0.
float filmic_curve(float light, float delta) {
    if (isnan(light) || light <= 0.0) {
        return 0.0;
    }
    vec4 grey = tone_base.rows[FILMIC_ROW_CURVE_GREY];
    vec4 toe = tone_base.rows[FILMIC_ROW_CURVE_TOE];
    vec4 powers = tone_base.rows[FILMIC_ROW_CURVE_POWERS];
    vec4 shoulder = tone_base.rows[FILMIC_ROW_CURVE_SHOULDER];
    float slope = grey.w;
    float stops = log2(light / grey.x);
    if (stops >= delta) {
        return 1.0;
    }
    float curve;
    if (stops > shoulder.x) {
        // Плечо: ширина и `r1^p1 − 1` зависят от белого точки, поэтому считаются здесь, а не заранее: пиксели вне
        // плеча (почти все) этих двух трансцендентных не платят.
        float rise = stops - shoulder.x;
        float width = delta - shoulder.x;
        float ratio = slope * width / shoulder.z;
        float excess = pow(ratio, powers.z) - 1.0;
        float blend = pow(rise / width, powers.z) * excess;
        curve = shoulder.y + slope * rise / pow(1.0 + blend, powers.w);
    } else if (stops >= -toe.x) {
        curve = grey.y + slope * stops;
    } else {
        float depth = -toe.x - stops;
        if (depth >= toe.z) {
            return 0.0;
        }
        float blend = pow(depth / toe.z, powers.x) * toe.w;
        curve = toe.y - slope * depth / pow(1.0 + blend, powers.y);
    }
    return clamp(curve, 0.0, 1.0);
}

// Ступеней от серого до белого точки: Δ_w = log2(max(L, 1)/g).
float filmic_white_stops(float white) {
    return log2(max(white, FILMIC_MIN_WHITE_LEVEL) / tone_base.rows[FILMIC_ROW_CURVE_GREY].x);
}

// ----- Норма, обесцвечивание, вписывание ----------------------------------------------------------------------

// Норма пикселя N(v) для цветных норм (FilmicBase::norm_of); v ≥ 0.
float filmic_norm(vec3 light, float norm_code) {
    float largest = max(max(light.x, light.y), light.z);
    if (norm_code == FILMIC_NORM_LUMINANCE) {
        vec4 row = tone_base.rows[FILMIC_ROW_LUMINANCE];
        precise float luminance = (row.x * light.x + row.y * light.y) + row.z * light.z;
        return luminance;
    }
    if (norm_code == FILMIC_NORM_POWER) {
        if (largest <= 0.0) {
            return 0.0;
        }
        // (R³+G³+B³)/(R²+G²+B²) = m·Σu³/Σu² при u = c/m, m = max: куб не переполняется.
        vec3 unit = light / largest;
        precise float squares = (unit.x * unit.x + unit.y * unit.y) + unit.z * unit.z;
        precise float cubes = ((unit.x * unit.x) * unit.x + (unit.y * unit.y) * unit.y) + (unit.z * unit.z) * unit.z;
        precise float power_norm = largest * cubes / squares;
        return power_norm;
    }
    // MaxRgb (PerChannel сюда не попадает: у него нормы нет).
    return largest;
}

// Вес обесцвечивания в светах d ∈ [0, 1] по выходу кривой y: гладкая smootherstep (FilmicCurve::desaturation_weight).
float filmic_desaturation_weight(float curved, float onset) {
    float t = clamp((curved - onset) / (1.0 - onset), 0.0, 1.0);
    precise float weight = t * t * t * (t * (6.0 * t - 15.0) + 10.0);
    return weight;
}

// Матрицы OKLab W (Rec. 2020) из строк модели.
OklabMatrices filmic_oklab() {
    OklabMatrices matrices;
    for (uint row = 0u; row < 3u; ++row) {
        matrices.working_to_lms[row] = tone_base.rows[FILMIC_ROW_OKLAB + row];
        matrices.lms_prime_to_lab[row] = tone_base.rows[FILMIC_ROW_OKLAB + 3u + row];
        matrices.lab_to_lms_prime[row] = tone_base.rows[FILMIC_ROW_OKLAB + 6u + row];
        matrices.lms_to_working[row] = tone_base.rows[FILMIC_ROW_OKLAB + 9u + row];
    }
    return matrices;
}

// Обесцвечивание в светах без смены оттенка (FilmicBase::desaturate_towards_white): смесь в OKLab между цветом и
// белым той же светлоты `display`. d = 0 — цвет как есть, d = 1 — ровно (display, display, display).
vec3 filmic_desaturate_towards_white(vec3 coloured, float display, float weight) {
    if (weight <= 0.0) {
        return coloured;
    }
    if (weight >= 1.0) {
        return vec3(display);
    }
    OklabMatrices oklab = filmic_oklab();
    vec3 lab = oklab_to_lab(oklab, coloured);
    vec3 white_lab = oklab_to_lab(oklab, vec3(display));
    float keep = 1.0 - weight;
    precise float mixed_lightness = keep * lab.x + weight * white_lab.x;
    return oklab_to_working(oklab, vec3(mixed_lightness, keep * lab.y, keep * lab.z));
}

// Вписывает цвет в охват ВЫХОДА без смены цветности (FilmicBase::fit_to_output_gamut): если после W → выход какой-то
// канал выше 1 + допуск, весь цвет делится на наибольший канал. Устойчивость на границе: скачок на пороге —
// множитель 1/(1 + 1e-5), то есть не больше 1e-5 относительно, поэтому расхождение ветки CPU/GPU в последнем бите
// порога не даёт видимой разницы.
vec3 filmic_fit_to_output_gamut(vec3 display_working) {
    vec3 output_color = filmic_apply_matrix(FILMIC_ROW_WORKING_TO_OUTPUT, display_working);
    float largest = max(max(max(1.0, output_color.x), output_color.y), output_color.z);
    if (largest <= 1.0 + FILMIC_FIT_TOLERANCE) {
        return display_working;
    }
    return display_working / largest;
}

// ----- Основа целиком ----------------------------------------------------------------------------------------

// Основа filmic после входа (FilmicBase::map_scene_pixel без входа): `entering` — результат filmic_apply_input;
// `white` — свой белый точки L (решение 177). Возвращает линейный свет экрана в W.
vec3 filmic_apply_base(vec3 entering, float white) {
    if (color_tail_has_nan(entering)) {
        return vec3(0.0);
    }
    vec3 light = clamp(entering, vec3(0.0), vec3(FILMIC_MAX_SCENE_LIGHT));
    vec4 flags = tone_base.rows[FILMIC_ROW_FLAGS];
    vec4 grey = tone_base.rows[FILMIC_ROW_CURVE_GREY];
    float gamma = grey.z;
    float delta = filmic_white_stops(white);
    if (flags.y == FILMIC_NORM_PER_CHANNEL) {
        return vec3(
            pow(filmic_curve(light.x, delta), gamma),
            pow(filmic_curve(light.y, delta), gamma),
            pow(filmic_curve(light.z, delta), gamma));
    }
    float norm = filmic_norm(light, flags.y);
    if (norm <= 0.0) {
        return vec3(0.0);
    }
    float curved = filmic_curve(norm, delta);
    float display = pow(curved, gamma);
    float weight = filmic_desaturation_weight(curved, tone_base.rows[FILMIC_ROW_CURVE_SHOULDER].w);
    vec3 coloured = vec3(display * (light.x / norm), display * (light.y / norm), display * (light.z / norm));
    return filmic_fit_to_output_gamut(filmic_desaturate_towards_white(coloured, display, weight));
}

// Выход, шаг 1: W → цвета выхода (матрица).
vec3 filmic_working_to_output(vec3 display_working) {
    return filmic_apply_matrix(FILMIC_ROW_WORKING_TO_OUTPUT, display_working);
}

// Выход, шаг 2: мягкое сжатие охвата цветов выхода.
vec3 filmic_compress_output_gamut(vec3 output_color) {
    vec4 threshold_power = tone_base.rows[FILMIC_ROW_OUTPUT_THRESHOLD_POWER];
    vec4 scale_inverse_power = tone_base.rows[FILMIC_ROW_OUTPUT_SCALE_INVERSE_POWER];
    return color_tail_compress_gamut(
        output_color, threshold_power.xyz, scale_inverse_power.xyz, threshold_power.w, scale_inverse_power.w);
}

// Выход, шаг 3: обрезка 0…1.
vec3 filmic_clamp_output(vec3 output_color) {
    return clamp(output_color, vec3(0.0), vec3(1.0));
}

#endif
