// Выходной проход: линейный RGB камеры (после демозаика и, если включён, шумодава) → цвет → охват →
// тон → OETF → цель приёмника кадра. Формулы и порядок операций — isp-model:
//   color/stage.rs (две матрицы 3x3 подряд, как в эталоне: промежуточный XYZ D50 — та же точка округления),
//   gamut.rs (мягкое сжатие ACES RGC-подобное), tone.rs (лог-логистическая S-кривая с сохранением
//   оттенка), transfer.rs (sRGB OETF; та же кривая для Display P3).
// Включающий файл до #include объявляет `output_image` с квалификатором формата цели.

#define FRAME_PARAMS_BINDING 3
#include "quality_frame_params.glsl"
#include "quality_common.glsl"

layout(set = 0, binding = 0, std430) readonly buffer LinearRed { float values[]; } linear_red;
layout(set = 0, binding = 1, std430) readonly buffer LinearGreen { float values[]; } linear_green;
layout(set = 0, binding = 2, std430) readonly buffer LinearBlue { float values[]; } linear_blue;

// Строка матрицы · пиксель, слева направо без FMA — как PixelMatrix::apply.
float matrix_row(vec4 row, vec3 pixel) {
    precise float value = (row.x * pixel.x + row.y * pixel.y) + row.z * pixel.z;
    return value;
}

vec3 apply_matrix(vec4 rows[3], vec3 pixel) {
    return vec3(matrix_row(rows[0], pixel), matrix_row(rows[1], pixel), matrix_row(rows[2], pixel));
}

bool any_nan(vec3 pixel) {
    return isnan(pixel.x) || isnan(pixel.y) || isnan(pixel.z);
}

// Сжатие расстояния канала от серого (GamutCompression::compress_distance).
float compress_distance(uint channel, float distance) {
    float threshold = frame.gamut_threshold_power[channel];
    if (distance < threshold) {
        return distance;
    }
    float scale = frame.gamut_scale[channel];
    float power = frame.gamut_threshold_power.w;
    float normalized = (distance - threshold) / scale;
    return threshold + scale * normalized / pow(1.0 + pow(normalized, power), 1.0 / power);
}

// Сжатие охвата (GamutCompression::compress_pixel): NaN и неположительный «серый» → чёрный.
vec3 compress_gamut(vec3 pixel) {
    if (any_nan(pixel)) {
        return vec3(0.0);
    }
    float achromatic = max(max(pixel.x, pixel.y), pixel.z);
    if (achromatic <= 0.0) {
        return vec3(0.0);
    }
    vec3 compressed;
    for (uint channel = 0u; channel < 3u; ++channel) {
        float distance = (achromatic - pixel[channel]) / achromatic;
        float compressed_distance = compress_distance(channel, distance);
        compressed[channel] = max(achromatic - compressed_distance * achromatic, 0.0);
    }
    return compressed;
}

// Тон одного канала (ToneCurve::map_channel): отрицательный свет и NaN → 0.
float tone_channel(float scene) {
    if (isnan(scene) || scene <= 0.0) {
        return 0.0;
    }
    float powered = pow(scene, frame.tone.x);
    return min(powered / (powered + frame.tone.y) * frame.tone.z, 1.0);
}

// Тон пикселя с сохранением оттенка (ToneCurve::map_pixel). Порядок каналов по убыванию — устойчивая
// сортировка вставками (как sort_by эталона: при равенстве раньше идёт меньший индекс).
vec3 apply_tone(vec3 scene) {
    if (any_nan(scene)) {
        return vec3(0.0);
    }
    vec3 display = vec3(tone_channel(scene.x), tone_channel(scene.y), tone_channel(scene.z));
    uint first = 0u;
    uint second = 1u;
    uint third = 2u;
    if (scene[second] > scene[first]) {
        uint swapped = first; first = second; second = swapped;
    }
    if (scene[third] > scene[second]) {
        uint swapped = second; second = third; third = swapped;
        if (scene[second] > scene[first]) {
            swapped = first; first = second; second = swapped;
        }
    }
    float input_span = scene[first] - scene[third];
    float hue_preservation = frame.tone.w;
    if (input_span <= 0.0 || hue_preservation <= 0.0) {
        return display;
    }
    float middle_share = (scene[second] - scene[third]) / input_span;
    float restored_middle = display[third] + middle_share * (display[first] - display[third]);
    display[second] += hue_preservation * (restored_middle - display[second]);
    return display;
}

// sRGB OETF (transfer::srgb_encode): NaN → 0, вход обрезается в 0…1.
float encode_srgb(float linear_value) {
    if (isnan(linear_value)) {
        return 0.0;
    }
    float clamped = clamp(linear_value, 0.0, 1.0);
    if (clamped <= 0.0031308) {
        return 12.92 * clamped;
    }
    return 1.055 * pow(clamped, 1.0 / 2.4) - 0.055;
}

void main() {
    uint x;
    uint y;
    if (!pixel_coordinates(x, y)) {
        return;
    }
    uint index = y * geometry.width + x;
    vec3 camera_rgb = vec3(linear_red.values[index], linear_green.values[index], linear_blue.values[index]);
    vec3 xyz_d50 = apply_matrix(frame.camera_to_xyz, camera_rgb);
    vec3 scene_linear = apply_matrix(frame.xyz_to_scene, xyz_d50);
    vec3 display_linear = apply_tone(compress_gamut(scene_linear));
    vec3 encoded = vec3(encode_srgb(display_linear.x), encode_srgb(display_linear.y), encode_srgb(display_linear.z));
    imageStore(output_image, ivec2(x, y), vec4(encoded, 1.0));
}
