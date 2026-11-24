#version 450
#extension GL_GOOGLE_include_directive : require
// Группа стекла: квад по границам группы (форма + слияние + тень + вынос линзы + свечение).
// Вершинный шейдер читает самый мелкий уровень пирамиды фона: средняя яркость под группой (`group_mean_luma`) —
// свойство группы, а не пикселя. Барьеры пирамиды (`pyramid.rs`) покрывают обе стадии: Vertex и Fragment.
#include "ui_frame.glsl"
#include "ui_glass_data.glsl"

layout(location = 0) out vec2 v_logical;
layout(location = 1) flat out uint v_group;
layout(location = 2) flat out float v_group_mean_luma;

void main() {
    uint index = INSTANCE_INDEX;
    GlassGroup group = groups[index];
    vec2 logical = group.bounds.xy + quad_corner(gl_VertexIndex) * group.bounds.zw;

    v_logical = logical;
    v_group = index;
    v_group_mean_luma = group_mean_luma(group);
    gl_Position = logical_to_clip(logical);
}
