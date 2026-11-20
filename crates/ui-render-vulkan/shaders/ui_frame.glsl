// Общие push-константы кадра UI и переход «логические пиксели → физическая цель».
// Один и тот же блок (112 байт) у конвейеров фигур, глифов и стекла; Rust-сторона — `src/gpu/frame_push.rs`.
// Рисование идёт в ЛОГИЧЕСКИХ пикселях (портрет, начало слева сверху, y вниз); физическую цель поворачивает
// дисплей (preTransform), поэтому вершинный шейдер переводит логические координаты в физические.

layout(push_constant) uniform FrameParams {
    vec4 geometry;    // x, y: логический размер; z, w: физический размер цели
    vec4 placement;   // прямоугольник фона на логическом экране: x, y, ширина, высота
    vec4 light_misc;  // x, y: направление света; z: число уровней пирамиды; w: 1 — фон живой, 0 — Absent
    vec4 absent_fill; // цвет заливки вместо фона (Absent)
    vec4 extra;       // x: spread атласа SDF, текселей; y, z: размер содержимого фона, текселей
    uvec4 indices;    // x: четверти поворота цели; y: номер первого экземпляра пакета
    vec4 window;      // окно чтения фона: origin.xy, step.xy (SourceWindow::mapping)
} frame;

// Обратное к physical_to_logical из gpu-vulkan/shaders/image_compositor.frag.
vec2 logical_to_physical(vec2 logical) {
    vec2 physical_size = frame.geometry.zw;
    uint turns = frame.indices.x;
    if (turns == 1u) {
        return vec2(logical.y, physical_size.y - logical.x);
    } else if (turns == 2u) {
        return vec2(physical_size.x - logical.x, physical_size.y - logical.y);
    } else if (turns == 3u) {
        return vec2(physical_size.x - logical.y, logical.x);
    }
    return logical;
}

vec4 logical_to_clip(vec2 logical) {
    vec2 physical = logical_to_physical(logical);
    return vec4(physical / frame.geometry.zw * 2.0 - 1.0, 0.0, 1.0);
}

// Углы квада из gl_VertexIndex (0..5): два треугольника.
vec2 quad_corner(int vertex) {
    const vec2 corners[6] = vec2[6](
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    return corners[vertex];
}

// Номер экземпляра в SSBO (только вершинные шейдеры): базовый из push-констант + gl_InstanceIndex
// (кит рисует с firstInstance = 0).
#define INSTANCE_INDEX (frame.indices.y + uint(gl_InstanceIndex))
