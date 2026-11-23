// Экземпляр картинки (SSBO, std430, 6 × vec4 = 96 байт). Rust-сторона — `src/pack/image.rs`.
struct ImageInstance {
    vec4 bounds;  // охватывающий прямоугольник на логическом экране: x, y, ширина, высота
    vec4 inv0;    // обратная матрица, строка 0: a, b, tx, 0
    vec4 inv1;    // обратная матрица, строка 1: c, d, ty, 0
    vec4 geom;    // SDF формы обрезки (круг): половина ширины, половина высоты, радиус, показатель
    vec4 uv;      // x, y: единиц uv источника на локальный пиксель (Cover/Stretch); z: прозрачность; w: номер сэмплера
    vec4 source;  // x: цветовой договор источника (COLOR_SPACE_*); y, z, w: резерв
};

// Цветовой договор источника; значения — ui_draw::draw_list::ImageColorSpace::shader_code.
const uint COLOR_SPACE_SRGB_PICTURE = 0u;    // sRGB-кодировка, переводится в цвет экрана
const uint COLOR_SPACE_DISPLAY_ENCODED = 1u; // уже цвет экрана (живая картинка с GPU), прямая альфа

layout(std430, set = 0, binding = 0) readonly buffer ImageInstances {
    ImageInstance instances[];
};
