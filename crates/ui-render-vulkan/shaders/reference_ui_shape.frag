#version 450
// ЗАМОРОЖЕННАЯ КОПИЯ на коммите 896865d (до оптимизаций быстрых путей, сессия 16a). НЕ МЕНЯТЬ.
// Нужна только тесту `glass_fast_paths_exact`: он сравнивает бит-в-бит боевой шейдер с этим эталоном.
// Боевой рендер эту копию не использует (конвейер собирается только под `test-support`).
#extension GL_GOOGLE_include_directive : require
#include "ui_frame.glsl"
#include "reference_ui_field.glsl"
#include "ui_shape_data.glsl"

layout(location = 0) in vec2 v_logical;
layout(location = 1) flat in uint v_instance;
layout(location = 0) out vec4 out_color;  // premultiplied

// Ширина сглаживания края, px: расстояние уже в пикселях экрана, производные не нужны.
const float EDGE_AA_PX = 1.0;

// Формулы — ui_draw::draw_list::shade_shape_pixel (заливка, обводка, тень под ними, всё × opacity).
void main() {
    ShapeInstance shape = instances[v_instance];
    ScreenField field = member_field_at(v_logical, shape.inv0, shape.inv1, shape.geom);

    vec4 color = vec4(shape.fill.rgb * shape.fill.a, shape.fill.a) * coverage_ref(field.distance_px, EDGE_AA_PX);

    float stroke_width = shape.misc.x;
    if (stroke_width > 0.0) {
        // Обводка центрирована на границе, ширина — в локальных px: переводим в экранные делением на scale.
        float stroke_distance = (abs(field.distance_local) - 0.5 * stroke_width) / field.scale;
        vec4 stroke = vec4(shape.stroke.rgb * shape.stroke.a, shape.stroke.a) * coverage_ref(stroke_distance, EDGE_AA_PX);
        color = stroke + color * (1.0 - stroke.a);
    }

    if (shape.shadow.y > 0.0) {
        // Тень: та же фигура, сдвинутая вниз, с мягким краем шириной ±radius вокруг границы.
        ScreenField shifted = member_field_at(v_logical - vec2(0.0, shape.shadow.z), shape.inv0, shape.inv1, shape.geom);
        float radius = shape.shadow.x;
        float falloff;
        if (radius > 0.0) {
            float t = clamp(0.5 - shifted.distance_px / (2.0 * radius), 0.0, 1.0);
            falloff = t * t * (3.0 - 2.0 * t);
        } else {
            falloff = coverage_ref(shifted.distance_px, EDGE_AA_PX);
        }
        vec4 shadow = vec4(0.0, 0.0, 0.0, 1.0) * (falloff * shape.shadow.y);
        color = color + shadow * (1.0 - color.a);
    }
    out_color = color * shape.misc.y;
}
