#version 450
#extension GL_GOOGLE_include_directive : require
// Следующие ступени: источник — предыдущий уровень пирамиды (px экрана на его тексель — steps.z).
layout(set = 0, binding = 0) uniform sampler2D source_level;

vec3 source_color(vec2 screen_px);
#include "ui_blur_down.glsl"

vec3 source_color(vec2 screen_px) {
    vec2 uv = screen_px / (vec2(textureSize(source_level, 0)) * blur.steps.z);
    return textureLod(source_level, uv, 0.0).rgb;
}
