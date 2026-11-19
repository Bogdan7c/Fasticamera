#version 450
#extension GL_GOOGLE_include_directive : require
// Глифы и иконки: по экземпляру на глиф; квад в локальных координатах элемента, преобразование — в экземпляре.
#include "ui_frame.glsl"

struct GlyphInstance {
    vec4 rect;    // локальный прямоугольник: x, y, ширина, высота
    vec4 uv;      // область атласа: u_min, v_min, u_max, v_max
    vec4 forward0;  // прямая матрица, строка 0: a, b, tx, 0
    vec4 forward1;  // строка 1: c, d, ty, 0
    vec4 color;   // цвет (не premultiplied), альфа уже умножена на прозрачность элемента
};

layout(std430, set = 0, binding = 0) readonly buffer GlyphInstances {
    GlyphInstance instances[];
};

layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out vec4 v_color;

void main() {
    GlyphInstance glyph = instances[INSTANCE_INDEX];
    vec2 corner = quad_corner(gl_VertexIndex);
    vec2 local = glyph.rect.xy + corner * glyph.rect.zw;
    vec2 logical = vec2(glyph.forward0.x * local.x + glyph.forward0.y * local.y + glyph.forward0.z,
                        glyph.forward1.x * local.x + glyph.forward1.y * local.y + glyph.forward1.z);
    v_uv = mix(glyph.uv.xy, glyph.uv.zw, corner);
    v_color = glyph.color;
    gl_Position = logical_to_clip(logical);
}
