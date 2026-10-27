#version 450
#extension GL_GOOGLE_include_directive : require
// Предпросмотр RAW10: кадр — упакованный буфер (4 пикселя в 5 байтах), читается как массив слов.
// Распаковка: пиксель i (0..3) группы = (byte_i << 2) | ((byte_4 >> (2*i)) & 3).

#include "raw_mosaic_params.glsl"

layout(set = 0, binding = 0, std430) readonly buffer PackedFrame {
    uint words[];
} packed_frame;

// Один байт буфера (little-endian внутри слова).
uint byte_at(uint byte_offset) {
    return (packed_frame.words[byte_offset >> 2u] >> ((byte_offset & 3u) * 8u)) & 0xFFu;
}

uint pixel_at(uint x, uint y) {
    uint group_offset = y * parameters.row_stride_bytes + (x >> 2u) * 5u;
    uint index_in_group = x & 3u;
    return (byte_at(group_offset + index_in_group) << 2u) | ((byte_at(group_offset + 4u) >> (2u * index_in_group)) & 3u);
}

uvec4 read_cell(uvec2 cell_origin) {
    return uvec4(
        pixel_at(cell_origin.x, cell_origin.y),
        pixel_at(cell_origin.x + 1u, cell_origin.y),
        pixel_at(cell_origin.x, cell_origin.y + 1u),
        pixel_at(cell_origin.x + 1u, cell_origin.y + 1u));
}

#include "raw_mosaic_main.glsl"
