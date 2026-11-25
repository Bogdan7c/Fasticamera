// Выходной проход: линейный RGB камеры (после демозаика и, если включён, шумодава) → цвет → охват →
// тон → OETF → цель приёмника кадра. Основа тона — по флагу блока AgX: прежняя (охват, S-кривая) или AgX со
// своим белым точки (решение 177; белый — по карте виньетирования в центре пикселя, выборка «по отсчёту»). Формулы и порядок операций — isp-model:
//   color/stage.rs (две матрицы 3x3 подряд, как в эталоне: промежуточный XYZ D50 — та же точка округления),
//   gamut.rs (мягкое сжатие ACES RGC-подобное), tone.rs (лог-логистическая S-кривая с сохранением
//   оттенка), transfer.rs (sRGB OETF; та же кривая для Display P3).
// Включающий файл до #include объявляет `output_image` с квалификатором формата цели.

#define FRAME_PARAMS_BINDING 3
// Блок основы тона (привязка 5, любая модель), карта виньетирования (привязка 6), цвет вида (привязка 7) и
// таблица вида (привязка 8): общие для видоискателя, качественного пути и снимка.
#define TONE_BASE_PARAMS_BINDING 5
#define LENS_SHADING_MAP_BINDING 6
#define LOOK_PARAMS_BINDING 7
#define LOOK_LUT_BINDING 8
#include "quality_frame_params.glsl"
#include "quality_common.glsl"
// Общий хвост цвета (охват, тон, OETF): те же формулы, что в слитом проходе видоискателя.
#include "color_tail.glsl"
#include "lens_shading.glsl"
#include "tone_base_agx.glsl"
#include "tone_base_aces.glsl"
#include "tone_base_filmic.glsl"
// Слой вида над светом экрана после основы (цвет вида, таблица); от модели основы не зависит.
#include "look_stage.glsl"

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

// Охват, тон и OETF — общие формулы `color_tail.glsl`; здесь только выбор параметров из `frame`.
// Обратная степень колена считается на GPU (`1.0 / power`), как и до выноса формул в общий файл.
vec3 compress_gamut(vec3 pixel) {
    float power = frame.gamut_threshold_power.w;
    return color_tail_compress_gamut(
        pixel, frame.gamut_threshold_power.xyz, frame.gamut_scale.xyz, power, 1.0 / power);
}

vec3 apply_tone(vec3 scene) {
    return color_tail_apply_tone(scene, frame.tone);
}

// Свой белый точки L пикселя (isp_model::white_level::WhiteLevelField::level_at, выборка «по отсчёту»):
// `valid_pixel` — координаты пикселя в ПОЛЕЗНОЙ области кадра. Нет коррекции — наименьшее усиление баланса.
float pixel_white_level(uvec2 valid_pixel) {
    vec4 map_gains = vec4(1.0);
    if (lens_shading_enabled()) {
        map_gains = lens_shading_gains(lens_shading_axis_x(valid_pixel.x), lens_shading_axis_y(valid_pixel.y));
    }
    return lens_shading_white_level(map_gains, tone_base_white_balance());
}

// Закодированный сигнал экрана пикселя `index` плоскостей. `valid_pixel` — его координаты в полезной области
// (нужны только основе AgX: белый точки по карте).
vec3 encoded_pixel(uint index, uvec2 valid_pixel) {
    vec3 camera_rgb = vec3(linear_red.values[index], linear_green.values[index], linear_blue.values[index]);
    vec3 xyz_d50 = apply_matrix(frame.camera_to_xyz, camera_rgb);
    vec3 display_linear;
    // Модель основы — из общего блока (качественный путь и снимок не пересоздают шейдер по модели).
    if (tone_base_model() == TONE_MODEL_AGX) {
        vec3 working_scene = agx_apply_matrix(AGX_ROW_XYZ_D50_TO_WORKING, xyz_d50);
        vec3 shaped = agx_apply_base(agx_apply_input(working_scene), pixel_white_level(valid_pixel));
        // Вид: тон, цвет, таблица — в W после основы; значения вне 0…1 принимает выходное сжатие охвата.
        vec3 looked = look_apply_display(shaped);
        display_linear = agx_clamp_output(agx_compress_output_gamut(agx_working_to_output(looked)));
    } else if (tone_base_model() == TONE_MODEL_FILMIC) {
        vec3 working_scene = filmic_apply_matrix(FILMIC_ROW_XYZ_D50_TO_WORKING, xyz_d50);
        vec3 shaped = filmic_apply_base(filmic_apply_input(working_scene), pixel_white_level(valid_pixel));
        vec3 looked = look_apply_display(shaped);
        display_linear = filmic_clamp_output(filmic_compress_output_gamut(filmic_working_to_output(looked)));
    } else if (tone_base_model() == TONE_MODEL_ACES) {
        vec3 working_scene = aces_apply_matrix(ACES_ROW_XYZ_D50_TO_WORKING, xyz_d50);
        vec3 shaped = aces_apply_base(aces_apply_input(working_scene), pixel_white_level(valid_pixel));
        vec3 looked = look_apply_display(shaped);
        display_linear = aces_clamp_output(aces_compress_output_gamut(aces_working_to_output(looked)));
    } else {
        vec3 scene_linear = apply_matrix(frame.xyz_to_scene, xyz_d50);
        // Прежняя основа: W = цвета выхода; вид — над светом экрана после S-кривой (0…1 обрежет OETF).
        display_linear = look_apply_display(apply_tone(compress_gamut(scene_linear)));
    }
    return color_tail_encode_output(display_linear);
}

void main() {
    uint x;
    uint y;
    if (!pixel_coordinates(x, y)) {
        return;
    }
    uint index = y * geometry.width + x;
    // Плоскости стенда — вся полезная область кадра: координаты пикселя в ней — его собственные.
    vec3 encoded = encoded_pixel(index, uvec2(x, y));
    imageStore(output_image, ivec2(x, y), vec4(encoded, 1.0));
}
