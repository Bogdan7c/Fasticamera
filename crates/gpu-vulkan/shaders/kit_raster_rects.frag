#version 450
// Тестовый фрагментный шейдер растрового набора: пишет цвет экземпляра как есть.

layout(location = 0) flat in vec4 in_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = in_color;
}
