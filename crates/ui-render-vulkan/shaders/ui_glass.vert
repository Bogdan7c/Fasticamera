#version 450
#extension GL_GOOGLE_include_directive : require
// Группа стекла: квад по границам группы (форма + слияние + тень + вынос линзы + свечение).
// Пирамиду фона вершинный шейдер НЕ читает: все барьеры пирамиды рассчитаны на читателя Fragment
// (среднюю яркость под группой считает фрагментный шейдер, см. `ui_glass.frag`).
#include "ui_frame.glsl"
#include "ui_glass_data.glsl"

layout(location = 0) out vec2 v_logical;
layout(location = 1) flat out uint v_group;

void main() {
    uint index = INSTANCE_INDEX;
    GlassGroup group = groups[index];
    vec2 logical = group.bounds.xy + quad_corner(gl_VertexIndex) * group.bounds.zw;

    v_logical = logical;
    v_group = index;
    gl_Position = logical_to_clip(logical);
}
