#version 450
// Полноэкранный треугольник без вершинных буферов: три вершины покрывают весь viewport.
// Вершины (-1,-1), (3,-1), (-1,3) в clip-пространстве; часть за пределами экрана отсекается.

void main() {
    // gl_VertexIndex: 0, 1, 2 -> (0,0), (2,0), (0,2) -> clip (-1,-1), (3,-1), (-1,3).
    vec2 corner = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
