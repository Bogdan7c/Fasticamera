#version 450
// Тестовый фрагментный шейдер растрового набора: читает текстуру сэмплером в точке, соответствующей
// центру пикселя цели (uv = gl_FragCoord / размер цели). Вершины даёт fullscreen_triangle.vert.
// Если размер цели равен размеру текстуры, выборка попадает ровно в центры текселей (значения
// «как загружены»); если цель вдвое меньше — в центр блока 2×2 (билинейное среднее: ступень пирамиды
// размытия). Значение текстуры пишется как есть (R8: красный канал = значение, остальные 0, альфа 1).

layout(set = 0, binding = 0) uniform sampler2D source_texture;

layout(push_constant) uniform Target {
    vec2 target_size;  // размер цели, пиксели
} target_info;

layout(location = 0) out vec4 out_color;

void main() {
    vec2 uv = gl_FragCoord.xy / target_info.target_size;
    out_color = texture(source_texture, uv);
}
