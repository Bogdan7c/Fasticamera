// ТОЛЬКО ДЛЯ ТЕСТОВ: заполняет storage-картинку значениями из буфера (по vec4 на пиксель, построчно).
// Нужно, потому что у `StorageImage` нет передачи данных с CPU: тесты приборов кладут на вход точные значения.
layout(local_size_x = 16, local_size_y = 8) in;

layout(set = 0, binding = 1, std430) readonly buffer Texels {
    vec4 values[];
} texels;

layout(push_constant) uniform Extent {
    uint width;
    uint height;
} extent;

void main() {
    uvec2 pixel = gl_GlobalInvocationID.xy;
    if (pixel.x >= extent.width || pixel.y >= extent.height) {
        return;
    }
    imageStore(target_image, ivec2(pixel), texels.values[pixel.y * extent.width + pixel.x]);
}
