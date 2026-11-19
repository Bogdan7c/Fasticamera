#version 450
// Тестовый фрагментный шейдер растрового набора БЕЗ привязок набора дескрипторов: один цвет из
// push-констант. Вершины даёт fullscreen_triangle.vert. Проверяет конвейер без раскладки набора.

layout(push_constant) uniform Solid {
    vec4 color;
} solid;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = solid.color;
}
