// Вход ISP видоискателя: RAW10 — упакованный буфер кадра (4 пикселя в 5 байтах), читается словами.
// Распаковка как у отладочной мозаики (`gpu-vulkan/shaders/raw_mosaic_preview_raw10.frag`):
// пиксель i (0..3) группы = (byte_i << 2) | ((byte_4 >> (2*i)) & 3). Шаг строки — в БАЙТАХ (uniform).
// Включается ПОСЛЕ `viewfinder_isp.glsl`: там объявлены параметры и прототип `read_raw`.
layout(set = 0, binding = 0, std430) readonly buffer PackedFrame {
    uint words[];
} packed_frame;

// Один байт буфера (little-endian внутри слова).
uint packed_byte_at(uint byte_offset) {
    return (packed_frame.words[byte_offset >> 2u] >> ((byte_offset & 3u) * 8u)) & 0xFFu;
}

uint read_raw(uint x, uint y) {
    uint group_offset = y * parameters.output_geometry.w + (x >> 2u) * 5u;
    uint index_in_group = x & 3u;
    return (packed_byte_at(group_offset + index_in_group) << 2u) |
           ((packed_byte_at(group_offset + 4u) >> (2u * index_in_group)) & 3u);
}
