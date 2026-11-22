// Экземпляр картинки (SSBO, std430, 5 × vec4 = 80 байт). Rust-сторона — `src/pack/image.rs`.
struct ImageInstance {
    vec4 bounds;  // охватывающий прямоугольник на логическом экране: x, y, ширина, высота
    vec4 inv0;    // обратная матрица, строка 0: a, b, tx, 0
    vec4 inv1;    // обратная матрица, строка 1: c, d, ty, 0
    vec4 geom;    // SDF формы обрезки (круг): половина ширины, половина высоты, радиус, показатель
    vec4 uv;      // x, y: единиц uv источника на локальный пиксель (Cover); z: прозрачность; w: номер сэмплера
};

layout(std430, set = 0, binding = 0) readonly buffer ImageInstances {
    ImageInstance instances[];
};
