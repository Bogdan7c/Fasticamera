#version 450
// Компоновщик картинки на цель показа (сессия 07): берёт готовое изображение (выход ISP видоискателя,
// уже закодированное кривой экрана) и кладёт его в прямоугольник ЛОГИЧЕСКИХ (прямых) координат цели.
// Значения не меняются: линейная выборка «как есть», цель — UNORM (никогда не _SRGB), поэтому
// повторного кодирования нет. Вне прямоугольника — цвет фона (letterbox).
//
// Повороты дисплея: цель физическая, дисплей поворачивает её на display_turns четвертей по часовой
// (preTransform). Поэтому физические координаты пикселя переводятся в логические так же, как в
// raw_mosaic_main.glsl, но в непрерывной форме (центры пикселей — x + 0.5).

layout(set = 0, binding = 0) uniform sampler2D source_image;

layout(push_constant) uniform CompositeParameters {
    uint target_width;    // физическая ширина цели, пиксели
    uint target_height;   // физическая высота цели, пиксели
    uint display_turns;   // 0..3 четверти по часовой (SurfaceTransform)
    uint reserved;        // выравнивание vec4 ниже до 16 байт
    vec4 destination;     // x, y, ширина, высота прямоугольника в логических пикселях
    vec4 background;      // цвет вне прямоугольника
    vec4 window;          // окно источника (SourceWindow::mapping): origin.xy, step.xy — текселей
    vec4 content;         // x, y: размер содержимого источника, текселей (картинка может быть больше)
} parameters;

layout(location = 0) out vec4 out_color;

vec2 physical_to_logical(vec2 physical) {
    float width = float(parameters.target_width);
    float height = float(parameters.target_height);
    if (parameters.display_turns == 1u) {
        return vec2(height - physical.y, physical.x);
    } else if (parameters.display_turns == 2u) {
        return vec2(width - physical.x, height - physical.y);
    } else if (parameters.display_turns == 3u) {
        return vec2(physical.y, width - physical.x);
    }
    return physical;
}

void main() {
    vec2 logical = physical_to_logical(gl_FragCoord.xy);
    vec2 relative = (logical - parameters.destination.xy) / parameters.destination.zw;
    if (any(lessThan(relative, vec2(0.0))) || any(greaterThanEqual(relative, vec2(1.0)))) {
        out_color = parameters.background;
        return;
    }
    // Окно источника: содержимое в левом верхнем углу картинки, зеркало и увеличение (SourceWindow). Выборка
    // зажимается на полтекселя внутрь содержимого, чтобы билинейный фильтр не подмешал чужие текселя.
    vec2 texel = parameters.window.xy + relative * parameters.window.zw;
    texel = clamp(texel, vec2(0.5), max(parameters.content.xy - 0.5, vec2(0.5)));
    out_color = textureLod(source_image, texel / vec2(textureSize(source_image, 0)), 0.0);
}
