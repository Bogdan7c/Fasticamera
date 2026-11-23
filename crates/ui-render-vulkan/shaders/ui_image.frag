#version 450
#extension GL_GOOGLE_include_directive : require
#include "ui_frame.glsl"
#include "ui_field.glsl"
#include "ui_image_data.glsl"

// Сэмплеры картинок кадра: по привязке на текстуру (число = `MAX_IMAGE_BINDINGS` в config.rs). Формат картинок
// из памяти — Rgba8Unorm, НЕ sRGB: выборка идёт по закодированным значениям источника; в привязку может лечь и
// живая картинка с GPU (Rgba16Float, цвет экрана). Неиспользуемые привязки
// держат текстуру-заглушку 1×1.
layout(set = 0, binding = 1) uniform sampler2D picture_0;
layout(set = 0, binding = 2) uniform sampler2D picture_1;
layout(set = 0, binding = 3) uniform sampler2D picture_2;
layout(set = 0, binding = 4) uniform sampler2D picture_3;

// Основные цвета экрана (константа специализации; значения — как в Rust `DisplayPrimaries`): 0 — sRGB, 1 — Display P3.
layout(constant_id = 0) const uint DISPLAY_PRIMARIES = 0u;

layout(location = 0) in vec2 v_logical;
layout(location = 1) flat in uint v_instance;
layout(location = 0) out vec4 out_color;  // premultiplied

// Ширина сглаживания края, px (как у фигур).
const float EDGE_AA_PX = 1.0;

// Линейный sRGB → линейный Display P3: ui_draw::color::DisplayPrimaries::from_srgb_matrix (строки). Выведена из
// координат цветности (EG 432-1, IEC 61966-2-1, D65); сверяется тестом паритета и тестом вывода в ui-draw.
const vec3 SRGB_TO_P3_ROW_0 = vec3(0.82246197, 0.17753803, 0.0);
const vec3 SRGB_TO_P3_ROW_1 = vec3(0.0331942, 0.9668058, 0.0);
const vec3 SRGB_TO_P3_ROW_2 = vec3(0.01708263, 0.07239744, 0.91051993);

// Эталон srgb_decode / srgb_encode (ui-draw color/primaries.rs), по компонентам.
vec3 srgb_decode(vec3 encoded) {
    return mix(pow((encoded + 0.055) / 1.055, vec3(2.4)), encoded / 12.92, lessThanEqual(encoded, vec3(0.04045)));
}

vec3 srgb_encode(vec3 linear) {
    vec3 clamped = clamp(linear, 0.0, 1.0);
    return mix(1.055 * pow(clamped, vec3(1.0 / 2.4)) - 0.055, clamped * 12.92, lessThanEqual(clamped, vec3(0.0031308)));
}

// Эталон srgb_picture_to_display: sRGB-экран — значения как есть; P3 — кривая → матрица → кривая.
vec3 picture_to_display(vec3 encoded) {
    if (DISPLAY_PRIMARIES == 0u) {
        return encoded;
    }
    vec3 linear = srgb_decode(encoded);
    vec3 converted = vec3(dot(SRGB_TO_P3_ROW_0, linear), dot(SRGB_TO_P3_ROW_1, linear), dot(SRGB_TO_P3_ROW_2, linear));
    return srgb_encode(converted);
}

// Выбор текстуры по номеру; textureLod с уровнем 0: производные внутри ветвления не определены.
vec4 sample_picture(uint index, vec2 uv) {
    switch (index) {
        case 0u: return textureLod(picture_0, uv, 0.0);
        case 1u: return textureLod(picture_1, uv, 0.0);
        case 2u: return textureLod(picture_2, uv, 0.0);
        default: return textureLod(picture_3, uv, 0.0);
    }
}

// Формулы — ui_draw::draw_list::shade_image_pixel_with_live: покрытие формы обрезки · билинейная выборка (значения
// источника) · перевод в цвет экрана (только sRGB-картинки) · предумножение на альфу источника × покрытие × прозрачность.
void main() {
    ImageInstance image = instances[v_instance];
    ScreenField field = member_field_at(v_logical, image.inv0, image.inv1, image.geom);
    float clip_coverage = coverage_ref(field.distance_px, EDGE_AA_PX);

    vec2 local = vec2(image.inv0.x * v_logical.x + image.inv0.y * v_logical.y + image.inv0.z,
                      image.inv1.x * v_logical.x + image.inv1.y * v_logical.y + image.inv1.z);
    vec2 uv = 0.5 + local * image.uv.xy;
    vec4 texel = sample_picture(uint(image.uv.w + 0.5), uv);

    // Живая картинка (приборы цвета) уже в цвете экрана с прямой альфой: перевода нет, иначе на P3-экране
    // цвет «переехал» бы второй раз. Остальное (предумножение на альфу · покрытие · прозрачность) общее.
    vec3 display_rgb = (uint(image.source.x + 0.5) == COLOR_SPACE_DISPLAY_ENCODED) ? texel.rgb : picture_to_display(texel.rgb);
    float alpha = texel.a * clip_coverage * image.uv.z;
    out_color = vec4(display_rgb * alpha, alpha);
}
