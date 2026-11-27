// Блок параметров показа цветовых приборов — общий для обоих проходов показа (решение 322):
//   `scope_resample.comp` — перенос сетки счётчиков в столбцы картинки (по горизонтали);
//   `scope_render.comp`   — перенос по вертикали, цвет следа и сетка.
// Раскладка std430 — та же, что собирает `isp-vulkan/src/scopes/parameter_blocks.rs` (`render_block_bytes`); длины
// массивов сверяет тест `shader_literals_tests`.

// Примитив сетки: geometry — отрезок (from, to), кольцо (центр, радиусы) или рамка (центр, половины сторон);
// style.x — полуширина линии в пикселях, style.y — вид (0 отрезок, 1 кольцо, 2 рамка).
struct GraticulePrimitive {
    vec4 geometry;
    vec4 style;
};

layout(set = 0, binding = 1, std430) readonly buffer RenderParameters {
    uvec4 grid;             // столбцы, строки, плоскости сетки счётчиков, число примитивов
    vec4 trace;             // x = готовый знаменатель яркости log2(1 + порог): считает scope-model (`TraceReference`)
                            //     по плотности полной яркости и числу отсчётов кадра, шейдер порог не знает;
                            // y = непрозрачность следа, z = непрозрачность сетки,
                            // w = способ рисования следа (PAINT_*, код по `parameter_blocks.rs`)
    vec4 graticule_color;   // rgb
    vec4 plane_colors[3];   // rgb следа по плоскостям (секции) или основные цвета наложения
    GraticulePrimitive primitives[48];
    // ХВОСТ «цвет по положению» (способ PAINT_COLOR_BY_POSITION; у остальных способов — нули и не читается).
    vec4 position_palette[128]; // rgb края и радиус края (клеток) по углам круга
    vec4 position_neutral;      // rgb центра
    vec4 position_geometry;     // x = центр (клеток), y = радиус «это центр», z = наименьший радиус края
    // ПЕРЕНОС СЕТКИ В ПИКСЕЛИ (эталон — `scope_model::trace_resample` и `scope_model::trace_density`).
    uvec4 picture;              // ширина и высота картинки, слотов плоскостей на пиксель, 0
    // Ядра по осям — числа эталона `AxisResampling`: x = пикселей на клетку, y = 1 / (2σ²), z = высота гауссианы,
    // w = радиус ядра в пикселях. [0] — вертикаль, [1] — вся ширина, [2 + k] — секция плоскости k.
    vec4 resample_axes[5];
} parameters;

const uint AXIS_VERTICAL = 0u;
const uint AXIS_FULL_WIDTH = 1u;
const uint AXIS_FIRST_SECTION = 2u;

// Способы рисования следа (parameters.trace.w); те же числа, что `PAINT_CODE_*` в `parameter_blocks.rs`.
const float PAINT_PLANE_SECTIONS = 0.0;
const float PAINT_PLANES_OVERLAID = 1.0;
const float PAINT_COLOR_BY_POSITION = 2.0;

// Клетки оси, которые видит пиксель: полуоткрытый диапазон [x, y) в пределах 0..cells — `AxisResampling::cell_range`
// (центр клетки i — в (i + 0,5) · ppc; клетка нужна, если до центра пикселя не дальше радиуса ядра).
uvec2 axis_cell_range(vec4 axis, uint pixel, uint cells) {
    float centre = float(pixel) + 0.5;
    float lowest = ceil((centre - axis.w) / axis.x - 0.5);
    float highest = floor((centre + axis.w) / axis.x - 0.5);
    uint first = uint(max(lowest, 0.0));
    uint end = uint(clamp(highest + 1.0, 0.0, float(cells)));
    return uvec2(first, max(end, first));
}

// Вес клетки `cell` для пикселя `pixel` — `AxisResampling::weight`: тот же порядок операций f32.
float axis_weight(vec4 axis, uint cell, uint pixel) {
    float distance = (float(cell) + 0.5) * axis.x - (float(pixel) + 0.5);
    return axis.z * exp(-(distance * distance) * axis.y);
}

// Секция пикселя `x` картинки шириной `width` на `planes` секций — `scope_model::Section::of_pixel`:
// x = плоскость, y = первый пиксель секции, z = ширина секции.
uvec3 section_of_pixel(uint x, uint width, uint planes) {
    uint plane = (x * planes) / width;
    uint start = (plane * width + planes - 1u) / planes;
    uint end = ((plane + 1u) * width + planes - 1u) / planes;
    return uvec3(plane, start, end - start);
}
