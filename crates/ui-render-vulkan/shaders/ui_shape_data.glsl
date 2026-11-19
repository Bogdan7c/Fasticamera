// Экземпляр закрашенной фигуры (SSBO, std430, 8 × vec4 = 128 байт). Rust-сторона — `src/pack/shape.rs`.
struct ShapeInstance {
    vec4 bounds;  // охватывающий прямоугольник на логическом экране: x, y, ширина, высота
    vec4 inv0;    // обратная матрица, строка 0: a, b, tx, 0
    vec4 inv1;    // обратная матрица, строка 1: c, d, ty, 0
    vec4 geom;    // половина ширины, половина высоты, радиус, показатель
    vec4 fill;    // заливка (закодированные значения, не premultiplied)
    vec4 stroke;  // цвет обводки
    vec4 misc;    // x: ширина обводки (локальные px, 0 — нет); y: прозрачность элемента
    vec4 shadow;  // x: радиус; y: альфа (0 — нет тени); z: смещение вниз
};

layout(std430, set = 0, binding = 0) readonly buffer ShapeInstances {
    ShapeInstance instances[];
};
