// Общая часть проходов качественного конвейера полного кадра (стенд V4, сессия 07).
//
// Каждый проход работает над плоскостями f32 размера width x height (полезная область кадра) в
// device-local storage-буферах: одна нить = один пиксель. Геометрия и раскладка CFA приходят
// push-константами, размер рабочей группы — specialization-константами 0 и 1 (тот же размер Rust
// использует для подсчёта групп, см. gpu_vulkan::compute::LocalSize).
//
// Граничные правила — как у эталона isp-model (demosaic/planes.rs, colour-demosaicing/scipy):
// одномерные свёртки и сдвиги — «зеркало без повтора края» (d c b | a b c d | c b a).
//
// Арифметика объявлена `precise` там, где важен порядок округлений: компилятор не сливает a*b+c в
// FMA и не переставляет сложения, поэтому сумма считается в том же порядке, что на CPU.

layout(local_size_x_id = 0, local_size_y_id = 1, local_size_z = 1) in;

// Push-константы всех проходов (32 байта, раскладку пишет Rust: quality::pass::PassPushConstants):
// размер полезной области, код раскладки CFA (0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR), слово режима и четыре
// числа параметров прохода (смысл режима и чисел — у прохода; демозаик их не использует).
layout(push_constant) uniform PlaneGeometry {
    uint width;
    uint height;
    uint cfa_code;
    uint pass_mode;
    vec4 pass_parameters;
} geometry;

const uint CHANNEL_RED = 0u;
const uint CHANNEL_GREEN = 1u;
const uint CHANNEL_BLUE = 2u;

// Направление решения Menon 2007 в плоскости направлений: 0 — по строке, 1 — по столбцу.
const float DIRECTION_HORIZONTAL = 0.0;
const float DIRECTION_VERTICAL = 1.0;

// Канал фильтра в пикселе (x, y): таблица по раскладкам, позиции квадрата 2x2 в порядке строк.
uint channel_at(uint x, uint y) {
    const uint CHANNELS[16] = uint[16](
        0u, 1u, 1u, 2u,
        1u, 0u, 2u, 1u,
        1u, 2u, 0u, 1u,
        2u, 1u, 1u, 0u);
    return CHANNELS[geometry.cfa_code * 4u + (y & 1u) * 2u + (x & 1u)];
}

// Строка y содержит красные пиксели (а не синие) — как BayerPattern::row_has_red.
bool row_has_red(uint y) {
    return channel_at(0u, y) == CHANNEL_RED || channel_at(1u, y) == CHANNEL_RED;
}

// Зеркало без повтора края: индекс в 0..length-1 (length >= 2, проверено на CPU).
// Функция симметрична (mirror(-k) == mirror(k)), поэтому берём модуль — так остаток от деления
// считается только для неотрицательных чисел (для отрицательных `%` в GLSL не определён).
uint mirror_index(int index, uint length) {
    int period = 2 * (int(length) - 1);
    int folded = abs(index) % period;
    return uint(folded >= int(length) ? period - folded : folded);
}

// Линейный индекс соседа (x + dx, y + dy) с зеркальной границей.
uint mirrored_index(uint x, uint y, int dx, int dy) {
    uint source_x = mirror_index(int(x) + dx, geometry.width);
    uint source_y = mirror_index(int(y) + dy, geometry.height);
    return source_y * geometry.width + source_x;
}

// Шаг вдоль направления: по строке (1, 0) или по столбцу (0, 1).
ivec2 direction_step(float direction) {
    return direction == DIRECTION_HORIZONTAL ? ivec2(1, 0) : ivec2(0, 1);
}

// Направление интерполяции R/B в зелёном узле строки y (как chroma_direction_at_green эталона):
// нужный цвет лежит по строке, если строка «его», иначе — по столбцу.
float chroma_direction_at_green(uint y, uint wanted_channel) {
    bool row_has_wanted = row_has_red(y) == (wanted_channel == CHANNEL_RED);
    return row_has_wanted ? DIRECTION_HORIZONTAL : DIRECTION_VERTICAL;
}

// Координаты нити; false — нить вне плоскости (крайние группы dispatch выходят за край).
bool pixel_coordinates(out uint x, out uint y) {
    x = gl_GlobalInvocationID.x;
    y = gl_GlobalInvocationID.y;
    return x < geometry.width && y < geometry.height;
}

// Деление с правильным округлением (`divide_correctly_rounded`) — общий файл: им же пользуется коррекция
// виньетирования (lens_shading.glsl), где координата узла сетки обязана совпасть с CPU бит в бит.
#include "correctly_rounded_divide.glsl"
