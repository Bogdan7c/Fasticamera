// Цветовой хвост вывода V4 (сессия 16a, пакет V): матрицы камеры → основа тона → вид → охват → OETF для ОДНОГО пикселя.
// Отдельный от `v4_output_main.glsl` файл: тут только формулы цвета одного пикселя, там привязки плоскостей и main. Копия `quality_output_main.glsl` с маской отключаемых стадий
// (specialization-константа 2) и зафиксированной моделью основы (константа 3); продукт и снимок берут прежний файл.
//
// Включающий файл ДО этого #include: задаёт номера привязок (FRAME_PARAMS_BINDING, TONE_BASE_PARAMS_BINDING,
// LENS_SHADING_MAP_BINDING, LOOK_PARAMS_BINDING, LOOK_LUT_BINDING), подключает quality_frame_params.glsl и
// quality_common.glsl. Этот файл подключает остальной цвет сам.

#include "color_tail.glsl"
#include "lens_shading.glsl"
#include "tone_base_agx.glsl"
#include "tone_base_aces.glsl"
#include "tone_base_filmic.glsl"
#include "look_stage.glsl"

// Маска пропускаемых стадий (0 — полный вывод, как в продукте). Каждый бит — одна стадия ветки AgX:
const uint SKIP_CAMERA_MATRIX = 1u;   // матрица камера → XYZ D50 (вместо неё — сами отсчёты камеры)
const uint SKIP_TONE_BASE = 2u;       // основа тона AgX (кривая, возврат оттенка)
const uint SKIP_LOOK = 4u;            // цвет вида и таблица вида
const uint SKIP_OUTPUT_GAMUT = 8u;    // выходное сжатие охвата
const uint SKIP_OETF = 16u;           // кодирование OETF (на выход идёт линейный свет)
layout(constant_id = 2) const uint STAGE_SKIP_MASK = 0u;
// Модель основы тона, зафиксированная при создании пайплайна (specialization-константа 3): номер модели
// (`TONE_MODEL_*`) — драйвер выбрасывает остальные ветки и их код не занимает регистры; ANY_TONE_MODEL — модель
// читается из блока основы на лету, как в продукте.
const uint ANY_TONE_MODEL = 0xFFFFFFFFu;
layout(constant_id = 3) const uint FIXED_TONE_MODEL = ANY_TONE_MODEL;
uint output_tone_model() {
    return FIXED_TONE_MODEL == ANY_TONE_MODEL ? tone_base_model() : FIXED_TONE_MODEL;
}

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

// Закодированный сигнал экрана для цвета камеры `camera_rgb`. `valid_pixel` — координаты пикселя в полезной области
// (нужны только основе AgX: белый точки по карте).
vec3 encoded_color(vec3 camera_rgb, uvec2 valid_pixel) {
    vec3 xyz_d50 = (STAGE_SKIP_MASK & SKIP_CAMERA_MATRIX) != 0u ? camera_rgb : apply_matrix(frame.camera_to_xyz, camera_rgb);
    vec3 display_linear;
    // Модель основы — из общего блока (качественный путь и снимок не пересоздают шейдер по модели).
    if (output_tone_model() == TONE_MODEL_AGX) {
        vec3 working_scene = agx_apply_matrix(AGX_ROW_XYZ_D50_TO_WORKING, xyz_d50);
        vec3 input_scene = agx_apply_input(working_scene);
        vec3 shaped = (STAGE_SKIP_MASK & SKIP_TONE_BASE) != 0u
            ? input_scene
            : agx_apply_base(input_scene, pixel_white_level(valid_pixel));
        // Вид: цвет, затем таблица — в W после основы; значения вне 0…1 принимает выходное сжатие охвата.
        vec3 looked = (STAGE_SKIP_MASK & SKIP_LOOK) != 0u ? shaped : look_apply_lut(look_apply_color(shaped));
        vec3 output_linear = agx_working_to_output(looked);
        display_linear = agx_clamp_output(
            (STAGE_SKIP_MASK & SKIP_OUTPUT_GAMUT) != 0u ? output_linear : agx_compress_output_gamut(output_linear));
    } else if (output_tone_model() == TONE_MODEL_FILMIC) {
        vec3 working_scene = filmic_apply_matrix(FILMIC_ROW_XYZ_D50_TO_WORKING, xyz_d50);
        vec3 shaped = filmic_apply_base(filmic_apply_input(working_scene), pixel_white_level(valid_pixel));
        vec3 looked = look_apply_lut(look_apply_color(shaped));
        display_linear = filmic_clamp_output(filmic_compress_output_gamut(filmic_working_to_output(looked)));
    } else if (output_tone_model() == TONE_MODEL_ACES) {
        vec3 working_scene = aces_apply_matrix(ACES_ROW_XYZ_D50_TO_WORKING, xyz_d50);
        vec3 shaped = aces_apply_base(aces_apply_input(working_scene), pixel_white_level(valid_pixel));
        vec3 looked = look_apply_lut(look_apply_color(shaped));
        display_linear = aces_clamp_output(aces_compress_output_gamut(aces_working_to_output(looked)));
    } else {
        vec3 scene_linear = apply_matrix(frame.xyz_to_scene, xyz_d50);
        // Прежняя основа: W = цвета выхода; вид — над светом экрана после S-кривой (0…1 обрежет OETF).
        display_linear = look_apply_lut(look_apply_color(apply_tone(compress_gamut(scene_linear))));
    }
    return (STAGE_SKIP_MASK & SKIP_OETF) != 0u ? display_linear : color_tail_encode_output(display_linear);
}
