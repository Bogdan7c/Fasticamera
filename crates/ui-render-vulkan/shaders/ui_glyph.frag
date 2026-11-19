#version 450
#extension GL_GOOGLE_include_directive : require
#include "ui_frame.glsl"

layout(set = 0, binding = 1) uniform sampler2D sdf_atlas;

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in vec4 v_color;
layout(location = 0) out vec4 out_color;  // premultiplied

void main() {
    // Кодирование атласа (ui-draw::atlas): байт 128 — край, внутри меньше; ±spread текселей = 128 ± 127.
    float texel_value = texture(sdf_atlas, v_uv).r * 255.0;
    float signed_distance_texels = (texel_value - 128.0) / 127.0 * frame.extra.x;
    // Сколько текселей атласа в одном пикселе экрана (по производным непрерывных координат).
    vec2 texel_position = v_uv * vec2(textureSize(sdf_atlas, 0));
    float texels_per_pixel = max(length(dFdx(texel_position)), length(dFdy(texel_position)));
    float distance_px = signed_distance_texels / max(texels_per_pixel, 1.0e-6);
    float coverage = clamp(0.5 - distance_px, 0.0, 1.0);
    out_color = vec4(v_color.rgb * v_color.a, v_color.a) * coverage;
}
