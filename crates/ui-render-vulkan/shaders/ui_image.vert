#version 450
#extension GL_GOOGLE_include_directive : require
// Картинки списка рисования: квад по охватывающему прямоугольнику, данные — из SSBO.
#include "ui_frame.glsl"
#include "ui_image_data.glsl"

layout(location = 0) out vec2 v_logical;
layout(location = 1) flat out uint v_instance;

void main() {
    uint index = INSTANCE_INDEX;
    ImageInstance image = instances[index];
    vec2 logical = image.bounds.xy + quad_corner(gl_VertexIndex) * image.bounds.zw;
    v_logical = logical;
    v_instance = index;
    gl_Position = logical_to_clip(logical);
}
