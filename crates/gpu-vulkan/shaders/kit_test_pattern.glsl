// Общая часть тестового compute-шейдера набора (сессия 07, только тесты). Включающий файл обязан
// до #include объявить привязку 0: `layout(set = 0, binding = 0, <формат>) uniform writeonly image2D target_image;`
// (формат — по StorageImageFormat::glsl_format_qualifier).
//
// Проверяет все части compute-набора одним проходом:
//   * specialization: размер группы (constant_id 0, 1) и режим узора (constant_id 2);
//   * uniform-буфер: цвета четвертей (binding 1);
//   * storage-буфер чтения-записи: счётчик записанных пикселей (binding 2);
//   * push-константы: размер картинки и синий канал градиента.

layout(local_size_x_id = 0, local_size_y_id = 1) in;
layout(constant_id = 2) const uint PATTERN_MODE = 0u; // 0 = четверти, 1 = градиент координат

layout(set = 0, binding = 1, std140) uniform QuadrantColors {
    vec4 colors[4]; // левая верхняя, правая верхняя, левая нижняя, правая нижняя
} quadrant_colors;

layout(set = 0, binding = 2, std430) buffer Statistics {
    uint written_pixels;
} statistics;

layout(push_constant) uniform PatternParameters {
    uint width;
    uint height;
    float gradient_blue;
} parameters;

void main() {
    uvec2 position = gl_GlobalInvocationID.xy;
    if (position.x >= parameters.width || position.y >= parameters.height) {
        return;
    }
    vec4 color;
    if (PATTERN_MODE == 0u) {
        uint quadrant = (position.x >= parameters.width / 2u ? 1u : 0u) + (position.y >= parameters.height / 2u ? 2u : 0u);
        color = quadrant_colors.colors[quadrant];
    } else {
        color = vec4(float(position.x) / float(parameters.width - 1u),
                     float(position.y) / float(parameters.height - 1u),
                     parameters.gradient_blue, 1.0);
    }
    imageStore(target_image, ivec2(position), color);
    atomicAdd(statistics.written_pixels, 1u);
}
