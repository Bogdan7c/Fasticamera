// Общий «хвост цвета» ISP: сжатие охвата → тон с сохранением оттенка → sRGB OETF. Одни и те же формулы
// используют ОБА пути: слитый проход видоискателя (`viewfinder_isp.glsl`) и выходной проход качественного
// конвейера и снимка (`quality_output_main.glsl`, его же берёт `photo_output_rgba8.comp`). Эталон на CPU —
// isp-model: gamut.rs (GamutCompression::compress_pixel), tone.rs (ToneCurve::map_pixel), transfer.rs
// (srgb_encode; та же кривая для Display P3).
//
// Правила файла:
//   * Функции ЧИСТЫЕ: параметры приходят аргументами, а не читаются из uniform-блока. Раскладки
//     uniform-блоков видоискателя (`parameters`) и качественного пути (`frame`) разные, и файл от них
//     не зависит: включающий шейдер сам достаёт значения из своего блока и передаёт сюда.
//   * Ни одна функция файла не объявлена `precise`, и это сознательно: в видоискателе `precise` не было
//     никогда (он ради скорости вправе сливать a*b+c в FMA), а в качественном пути `precise` стоит только
//     на матрицах цвета (`matrix_row` в `quality_output_main.glsl`), не на охвате, тоне и OETF. Общий
//     код компилируется так же, как раньше, в обоих путях; добавить сюда `precise` значило бы изменить
//     результат видоискателя. Если когда-нибудь понадобится `precise` — это отдельное осознанное
//     изменение с новым замером паритета.
//   * Включение/выключение стадий (специализация видоискателя `STAGE_*`) — забота включающего файла:
//     здесь стадии всегда «включены».
//   * Файл защищён от повторного включения (его берут и tone_base.glsl, и look_stage.glsl).
#ifndef COLOR_TAIL_GLSL
#define COLOR_TAIL_GLSL

// ===== Константы sRGB OETF (IEC 61966-2-1); те же числа, что в isp-model transfer.rs ===============

const float OETF_LINEAR_LIMIT = 0.0031308;
const float OETF_LINEAR_SLOPE = 12.92;
const float OETF_POWER_SCALE = 1.055;
const float OETF_POWER_OFFSET = 0.055;
const float OETF_INVERSE_EXPONENT = 1.0 / 2.4;
// Обратная функция (EOTF; isp-model transfer::srgb_decode): порог на стороне сигнала и показатель.
const float EOTF_ENCODED_LIMIT = 0.04045;
const float EOTF_POWER_EXPONENT = 2.4;

// ===== Сжатие охвата ===============================================================================

// Есть ли в цвете NaN хотя бы в одном канале.
bool color_tail_has_nan(vec3 color) {
    return isnan(color.x) || isnan(color.y) || isnan(color.z);
}

// Сжатие расстояния канала от «серого» (GamutCompression::compress_distance): ниже порога расстояние
// не меняется, выше — мягкое колено со степенью `power`. `inverse_power` = 1/power приходит снаружи:
// видоискатель берёт его из uniform (посчитано на CPU), качественный путь считает `1.0 / power` у вызова —
// как и раньше, до выноса формулы.
float color_tail_compress_distance(float distance, float threshold, float scale, float power, float inverse_power) {
    if (distance < threshold) {
        return distance;
    }
    float normalized = (distance - threshold) / scale;
    return threshold + scale * normalized / pow(1.0 + pow(normalized, power), inverse_power);
}

// Сжатие охвата пикселя (GamutCompression::compress_pixel): NaN и неположительный «серый» → чёрный.
// `thresholds` и `scales` — параметры по каналам R, G, B.
vec3 color_tail_compress_gamut(vec3 scene, vec3 thresholds, vec3 scales, float power, float inverse_power) {
    if (color_tail_has_nan(scene)) {
        return vec3(0.0);
    }
    float achromatic = max(max(scene.r, scene.g), scene.b);
    if (achromatic <= 0.0) {
        return vec3(0.0);
    }
    vec3 compressed;
    for (uint channel = 0u; channel < 3u; ++channel) {
        float distance = (achromatic - scene[channel]) / achromatic;
        float compressed_distance =
            color_tail_compress_distance(distance, thresholds[channel], scales[channel], power, inverse_power);
        compressed[channel] = max(achromatic - compressed_distance * achromatic, 0.0);
    }
    return compressed;
}

// ===== Тон =========================================================================================

// Тон одного канала (ToneCurve::map_channel): отрицательный свет и NaN → 0. `tone`: x — контраст c,
// y — s^c, z — 1/F(x_white), w — доля сохранения оттенка (здесь не используется).
float color_tail_tone_channel(float scene, vec4 tone) {
    if (isnan(scene) || scene <= 0.0) {
        return 0.0;
    }
    float powered = pow(scene, tone.x);
    return min(powered / (powered + tone.y) * tone.z, 1.0);
}

// Тон пикселя с сохранением оттенка (ToneCurve::map_pixel).
vec3 color_tail_apply_tone(vec3 scene, vec4 tone) {
    if (color_tail_has_nan(scene)) {
        return vec3(0.0);
    }
    vec3 display = vec3(
        color_tail_tone_channel(scene.r, tone),
        color_tail_tone_channel(scene.g, tone),
        color_tail_tone_channel(scene.b, tone));
    // Порядок каналов по убыванию, при равенстве — R, G, B (устойчивая сортировка эталона):
    // максимум — первый из наибольших, минимум — последний из наименьших.
    uint max_channel = 0u;
    if (scene.g > scene[max_channel]) { max_channel = 1u; }
    if (scene.b > scene[max_channel]) { max_channel = 2u; }
    uint min_channel = 2u;
    if (scene.g < scene[min_channel]) { min_channel = 1u; }
    if (scene.r < scene[min_channel]) { min_channel = 0u; }
    uint middle_channel = 3u - max_channel - min_channel;
    float input_span = scene[max_channel] - scene[min_channel];
    float hue_preservation = tone.w;
    if (input_span <= 0.0 || hue_preservation <= 0.0) {
        return display;
    }
    float middle_share = (scene[middle_channel] - scene[min_channel]) / input_span;
    float restored_middle = display[min_channel] + middle_share * (display[max_channel] - display[min_channel]);
    display[middle_channel] += hue_preservation * (restored_middle - display[middle_channel]);
    return display;
}

// ===== sRGB OETF ===================================================================================

// sRGB OETF одного канала (transfer::srgb_encode): NaN → 0, вход обрезается в 0…1.
float color_tail_srgb_encode(float linear_light) {
    if (isnan(linear_light)) {
        return 0.0;
    }
    float clamped = clamp(linear_light, 0.0, 1.0);
    if (clamped <= OETF_LINEAR_LIMIT) {
        return OETF_LINEAR_SLOPE * clamped;
    }
    return OETF_POWER_SCALE * pow(clamped, OETF_INVERSE_EXPONENT) - OETF_POWER_OFFSET;
}

// sRGB EOTF одного канала (transfer::srgb_decode): сигнал обрезается в 0…1 (NaN ведёт себя как у `clamp` GLSL — вход
// таблицы вида уже прошёл `color_tail_srgb_encode`, где NaN → 0).
float color_tail_srgb_decode(float encoded) {
    float clamped = clamp(encoded, 0.0, 1.0);
    if (clamped <= EOTF_ENCODED_LIMIT) {
        return clamped / OETF_LINEAR_SLOPE;
    }
    return pow((clamped + OETF_POWER_OFFSET) / OETF_POWER_SCALE, EOTF_POWER_EXPONENT);
}

// sRGB OETF трёх каналов.
vec3 color_tail_encode_srgb(vec3 display_linear) {
    return vec3(
        color_tail_srgb_encode(display_linear.r),
        color_tail_srgb_encode(display_linear.g),
        color_tail_srgb_encode(display_linear.b));
}

// Кодирование выхода по профилю (isp-model `output::OutputTransfer::encode`) — ЕДИНСТВЕННАЯ точка выбора кривой
// выхода (решение 184). Сейчас профиль один — sRGB (одна кривая для sRGB и Display P3); Log и HDR из плана `video`
// появятся здесь веткой по числу кривой из uniform (или константой специализации), а не правкой вызывающих.
// Таблица вида (`look_stage.glsl`) кодирует sRGB-кривой НЕЗАВИСИМО от профиля: её домен — свойство вида, а не выхода.
vec3 color_tail_encode_output(vec3 display_linear) {
    return color_tail_encode_srgb(display_linear);
}

#endif
