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

// Два соседних отсчёта (x, x+1) строки y, x чётное: обе позиции лежат в одной группе из 5 байт, а группа — не больше
// чем в двух соседних словах. Читаем эти два слова один раз вместо шести загрузок слов `read_raw` на два отсчёта
// и склеиваем байты сдвигами. Результат тот же, что у двух вызовов `read_raw`.
uvec2 read_raw_pair(uint x, uint y) {
    uint group_offset = y * parameters.output_geometry.w + (x >> 2u) * 5u;
    uint shift = (group_offset & 3u) * 8u;
    uint word_index = group_offset >> 2u;
    uint low_word = packed_frame.words[word_index];
    // Слово word_index + 1 есть всегда: группа заканчивается внутри буфера, а размер буфера кратен слову.
    uint high_word = packed_frame.words[word_index + 1u];
    // Байты 0..3 группы; сдвиг на 32 в GLSL не определён, поэтому нулевой сдвиг — отдельная ветка.
    uint bytes_0_to_3 = shift == 0u ? low_word : ((low_word >> shift) | (high_word << (32u - shift)));
    // Байт 4 группы (младшие 2 бита каждого из четырёх пикселей) лежит в старшем слове.
    uint byte_4 = (high_word >> shift) & 0xFFu;
    uint index_in_group = x & 3u;
    uint first = (((bytes_0_to_3 >> (8u * index_in_group)) & 0xFFu) << 2u) | ((byte_4 >> (2u * index_in_group)) & 3u);
    uint second_index = index_in_group + 1u;
    uint second = (((bytes_0_to_3 >> (8u * second_index)) & 0xFFu) << 2u) | ((byte_4 >> (2u * second_index)) & 3u);
    return uvec2(first, second);
}

uvec4 read_raw_cell(uvec2 origin) {
    uvec2 upper_row = read_raw_pair(origin.x, origin.y);
    uvec2 lower_row = read_raw_pair(origin.x, origin.y + 1u);
    return uvec4(upper_row, lower_row);
}
