// Выходной проход стенда V4: линейный RGB камеры из плоскостей → цветовой хвост (`v4_output_color.glsl`) → цель
// приёмника кадра. Включающий файл до #include объявляет `output_image` с квалификатором формата цели.

#define FRAME_PARAMS_BINDING 3
#define TONE_BASE_PARAMS_BINDING 5
#define LENS_SHADING_MAP_BINDING 6
#define LOOK_PARAMS_BINDING 7
#define LOOK_LUT_BINDING 8
#include "quality_frame_params.glsl"
#include "quality_common.glsl"
#include "v4_output_color.glsl"

// МАКЕТ СТАБИЛИЗАТОРА (specialization-константа 4, оценка цены, не продукт): при 1 выходной пиксель берёт цвет камеры
// не из своего узла, а из точки повёрнутого кадра со сдвигом строки — билинейная выборка трёх плоскостей (12 чтений
// вместо 3), дальше тот же хвост цвета. Так меряется цена полнокадровой перепроекции, слитой с проходом вывода.
// Числа макета — типичные для стабилизации: поворот 0,3° вокруг центра и «резиновый» сдвиг строки до 3 пикселей.
layout(constant_id = 4) const uint STABILIZER_MOCK = 0u;
const float MOCK_ROTATION_RADIANS = 0.005236;
const float MOCK_ROW_SHIFT_PIXELS = 3.0;

layout(set = 0, binding = 0, std430) readonly buffer LinearRed { float values[]; } linear_red;
layout(set = 0, binding = 1, std430) readonly buffer LinearGreen { float values[]; } linear_green;
layout(set = 0, binding = 2, std430) readonly buffer LinearBlue { float values[]; } linear_blue;

// Билинейная выборка трёх плоскостей в точке `source` (в пикселях кадра, центр узла — целое + 0,5), край зажат.
vec3 sample_camera_bilinear(vec2 source) {
    vec2 clamped = clamp(source - 0.5, vec2(0.0), vec2(float(geometry.width - 1u), float(geometry.height - 1u)));
    uvec2 base = uvec2(floor(clamped));
    vec2 fraction = clamped - vec2(base);
    uvec2 next = min(base + 1u, uvec2(geometry.width - 1u, geometry.height - 1u));
    uint index_00 = base.y * geometry.width + base.x;
    uint index_10 = base.y * geometry.width + next.x;
    uint index_01 = next.y * geometry.width + base.x;
    uint index_11 = next.y * geometry.width + next.x;
    vec3 pixel_00 = vec3(linear_red.values[index_00], linear_green.values[index_00], linear_blue.values[index_00]);
    vec3 pixel_10 = vec3(linear_red.values[index_10], linear_green.values[index_10], linear_blue.values[index_10]);
    vec3 pixel_01 = vec3(linear_red.values[index_01], linear_green.values[index_01], linear_blue.values[index_01]);
    vec3 pixel_11 = vec3(linear_red.values[index_11], linear_green.values[index_11], linear_blue.values[index_11]);
    return mix(mix(pixel_00, pixel_10, fraction.x), mix(pixel_01, pixel_11, fraction.x), fraction.y);
}

// Цвет камеры выходного пикселя: свой узел или (макет стабилизатора) точка повёрнутого кадра со сдвигом строки.
vec3 camera_rgb_for_output(uint index, uvec2 valid_pixel) {
    if (STABILIZER_MOCK == 0u) {
        return vec3(linear_red.values[index], linear_green.values[index], linear_blue.values[index]);
    }
    vec2 centre = 0.5 * vec2(float(geometry.width), float(geometry.height));
    vec2 offset = vec2(valid_pixel) + 0.5 - centre;
    float cosine = cos(MOCK_ROTATION_RADIANS);
    float sine = sin(MOCK_ROTATION_RADIANS);
    vec2 rotated = vec2(cosine * offset.x - sine * offset.y, sine * offset.x + cosine * offset.y);
    float row_shift = MOCK_ROW_SHIFT_PIXELS * sin(float(valid_pixel.y) * 0.01);
    return sample_camera_bilinear(centre + rotated + vec2(row_shift, 0.0));
}

void main() {
    uint x;
    uint y;
    if (!pixel_coordinates(x, y)) {
        return;
    }
    uint index = y * geometry.width + x;
    // Плоскости стенда — вся полезная область кадра: координаты пикселя в ней — его собственные.
    vec3 encoded = encoded_color(camera_rgb_for_output(index, uvec2(x, y)), uvec2(x, y));
    imageStore(output_image, ivec2(x, y), vec4(encoded, 1.0));
}
