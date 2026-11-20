// Окно чтения фона (gpu_vulkan::source_window::SourceWindow): содержимое кадра камеры лежит в левом верхнем
// углу картинки постоянной ёмкости; фон может быть зеркальным или увеличенным вокруг центра.
// texel = origin + relative · step, затем зажим на полтекселя внутрь содержимого (чтобы билинейный фильтр не
// подмешал чужие текселя ёмкости). Возвращает координаты для sampler2D (доли всей картинки).
// origin_step: origin.xy, step.xy; content: размер содержимого, текселей.
vec2 window_uv(vec2 relative, vec4 origin_step, vec2 content, vec2 image_size) {
    vec2 texel = origin_step.xy + relative * origin_step.zw;
    texel = clamp(texel, vec2(0.5), max(content - 0.5, vec2(0.5)));
    return texel / image_size;
}
