#version 450
// Тестовый вершинный шейдер растрового набора (сессия 11): рисует N прямоугольников за ОДИН вызов.
// Данные экземпляров лежат в storage-буфере; вершины генерируются из gl_VertexIndex (0..5 — два
// треугольника квада) и gl_InstanceIndex (номер прямоугольника). Вершинных буферов нет.

struct Instance {
    vec4 rect;   // x, y, ширина, высота в пикселях цели (начало — левый верхний угол)
    vec4 color;  // цвет, который фрагментный шейдер запишет как есть (для смешивания — premultiplied)
};

layout(std430, set = 0, binding = 0) readonly buffer Instances {
    Instance instances[];
};

layout(push_constant) uniform Target {
    vec2 target_size;  // размер цели, пиксели
} target_info;

layout(location = 0) flat out vec4 out_color;

void main() {
    // Углы квада: 0,1,2 и 2,1,3 -> (0,0) (1,0) (0,1) (0,1) (1,0) (1,1).
    const vec2 corners[6] = vec2[6](
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    Instance instance = instances[gl_InstanceIndex];
    vec2 pixel = instance.rect.xy + corners[gl_VertexIndex] * instance.rect.zw;
    // Пиксели -> clip: x 0..W -> -1..1, y 0..H -> -1..1 (ось Y Vulkan смотрит вниз: пиксель 0 — верх).
    gl_Position = vec4(pixel / target_info.target_size * 2.0 - 1.0, 0.0, 1.0);
    out_color = instance.color;
}
