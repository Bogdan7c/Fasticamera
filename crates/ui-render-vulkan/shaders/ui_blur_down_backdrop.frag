#version 450
#extension GL_GOOGLE_include_directive : require
// Первая ступень: источник — картинка фона, размещённая композитором в прямоугольнике destination.
// Вне прямоугольника фон чёрный (как у компоновщика). Результат — уровень 1 в системе координат экрана.
layout(set = 0, binding = 0) uniform sampler2D source_image;

vec3 source_color(vec2 screen_px);
#include "ui_blur_down.glsl"

vec3 source_color(vec2 screen_px) {
    vec2 relative = (screen_px - blur.destination.xy) / blur.destination.zw;
    if (any(lessThan(relative, vec2(0.0))) || any(greaterThanEqual(relative, vec2(1.0)))) {
        return vec3(0.0);
    }
    return textureLod(source_image, relative, 0.0).rgb;
}
